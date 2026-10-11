//! Pointer and touchpad settings (`input SETTING VALUE [DEVICE]`).
//!
//! river 0.4 leaves libinput configuration to clients of
//! river-libinput-config-v1. Settings are kept as rules and applied to every
//! matching device, including ones plugged in later. Devices that don't
//! support a setting (keyboards, for tap-to-click) just decline it.

use std::collections::{BTreeMap, HashMap};

use wayland_backend::client::ObjectId;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};

use crate::protocol::{
    river_input_device_v1::{self, RiverInputDeviceV1},
    river_libinput_config_v1::{self, RiverLibinputConfigV1},
    river_libinput_device_v1::{
        self as dev, AccelProfile, DragState, DwtState, LeftHandedState, MiddleEmulationState,
        NaturalScrollState, RiverLibinputDeviceV1, TapState,
    },
    river_libinput_result_v1::{self, RiverLibinputResultV1},
};
use crate::wm::Wm;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Setting {
    Tap,
    Drag,
    NaturalScroll,
    Dwt,
    LeftHanded,
    MiddleEmulation,
    AccelProfile,
    AccelSpeed,
}

const SETTINGS: &[(&str, Setting)] = &[
    ("tap", Setting::Tap),
    ("drag", Setting::Drag),
    ("natural_scroll", Setting::NaturalScroll),
    ("dwt", Setting::Dwt),
    ("left_handed", Setting::LeftHanded),
    ("middle_emulation", Setting::MiddleEmulation),
    ("accel_profile", Setting::AccelProfile),
    ("accel_speed", Setting::AccelSpeed),
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Value {
    On(bool),
    Profile(AccelProfile),
    Speed(f64),
}

#[derive(Clone, Debug)]
struct Rule {
    setting: Setting,
    value: Value,
    /// only devices whose name contains this (case-insensitive)
    device: Option<String>,
}

impl Rule {
    fn matches(&self, name: &str) -> bool {
        match &self.device {
            None => true,
            Some(d) => name.to_lowercase().contains(&d.to_lowercase()),
        }
    }
}

struct Device {
    obj: RiverLibinputDeviceV1,
    input: Option<ObjectId>,
    /// settings this device reported support for, with their current values
    state: BTreeMap<&'static str, String>,
    supported: Vec<&'static str>,
}

#[derive(Default)]
pub struct Inputs {
    pub config: Option<RiverLibinputConfigV1>,
    /// river_input_device_v1 -> device name
    names: HashMap<ObjectId, String>,
    devices: Vec<Device>,
    rules: Vec<Rule>,
}

fn parse_bool(v: &str) -> Result<bool, String> {
    match v {
        "on" | "true" | "1" | "enabled" => Ok(true),
        "off" | "false" | "0" | "disabled" => Ok(false),
        _ => Err(format!("invalid value \"{v}\" (on or off)")),
    }
}

fn parse(setting: Setting, v: &str) -> Result<Value, String> {
    Ok(match setting {
        Setting::AccelProfile => Value::Profile(match v {
            "flat" => AccelProfile::Flat,
            "adaptive" => AccelProfile::Adaptive,
            "none" => AccelProfile::None,
            _ => return Err(format!("invalid profile \"{v}\" (flat, adaptive or none)")),
        }),
        Setting::AccelSpeed => {
            let s: f64 = v
                .parse()
                .map_err(|_| format!("invalid speed \"{v}\" (-1 to 1)"))?;
            if !(-1.0..=1.0).contains(&s) {
                return Err(format!("speed {s} out of range (-1 to 1)"));
            }
            Value::Speed(s)
        }
        _ => Value::On(parse_bool(v)?),
    })
}

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}

fn raw<T: Into<u32>>(w: WEnum<T>) -> u32 {
    match w {
        WEnum::Value(v) => v.into(),
        WEnum::Unknown(u) => u,
    }
}

impl Inputs {
    fn name_of(&self, d: &Device) -> Option<&str> {
        d.input
            .as_ref()
            .and_then(|i| self.names.get(i))
            .map(String::as_str)
    }

    /// Lines of "NAME\tsetting=value ..." for `list_inputs`.
    pub fn list(&self) -> String {
        let mut out = String::new();
        for d in &self.devices {
            let Some(name) = self.name_of(d) else {
                continue;
            };
            let fields: Vec<String> = d
                .state
                .iter()
                .filter(|(k, _)| d.supported.contains(k))
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            if !fields.is_empty() {
                out.push_str(&format!("{name}\t{}\n", fields.join(" ")));
            }
        }
        out
    }

    fn apply(&self, d: &Device, rule: &Rule, qh: &QueueHandle<Wm>) {
        let Some(name) = self.name_of(d) else { return };
        if !rule.matches(name) {
            return;
        }
        let o = &d.obj;
        let label = (name.to_string(), rule.setting);
        match (rule.setting, rule.value) {
            (Setting::Tap, Value::On(on)) => {
                o.set_tap(
                    if on {
                        TapState::Enabled
                    } else {
                        TapState::Disabled
                    },
                    qh,
                    label,
                );
            }
            (Setting::Drag, Value::On(on)) => {
                o.set_drag(
                    if on {
                        DragState::Enabled
                    } else {
                        DragState::Disabled
                    },
                    qh,
                    label,
                );
            }
            (Setting::NaturalScroll, Value::On(on)) => {
                let s = if on {
                    NaturalScrollState::Enabled
                } else {
                    NaturalScrollState::Disabled
                };
                o.set_natural_scroll(s, qh, label);
            }
            (Setting::Dwt, Value::On(on)) => {
                o.set_dwt(
                    if on {
                        DwtState::Enabled
                    } else {
                        DwtState::Disabled
                    },
                    qh,
                    label,
                );
            }
            (Setting::LeftHanded, Value::On(on)) => {
                let s = if on {
                    LeftHandedState::Enabled
                } else {
                    LeftHandedState::Disabled
                };
                o.set_left_handed(s, qh, label);
            }
            (Setting::MiddleEmulation, Value::On(on)) => {
                let s = if on {
                    MiddleEmulationState::Enabled
                } else {
                    MiddleEmulationState::Disabled
                };
                o.set_middle_emulation(s, qh, label);
            }
            (Setting::AccelProfile, Value::Profile(p)) => {
                o.set_accel_profile(p, qh, label);
            }
            (Setting::AccelSpeed, Value::Speed(s)) => {
                o.set_accel_speed(s.to_ne_bytes().to_vec(), qh, label);
            }
            _ => {}
        }
    }

    /// Apply every rule to one device, in order, so later rules win.
    fn apply_all(&self, i: usize, qh: &QueueHandle<Wm>) {
        for r in &self.rules {
            self.apply(&self.devices[i], r, qh);
        }
    }
}

impl Wm {
    /// `input SETTING VALUE [DEVICE]`
    pub fn input_command(&mut self, args: &[String]) -> Result<(), String> {
        let usage = "usage: input SETTING VALUE [DEVICE]; settings: tap, drag, natural_scroll, \
                     dwt, left_handed, middle_emulation (on/off), accel_profile (flat/adaptive), \
                     accel_speed (-1 to 1)";
        let (Some(name), Some(value)) = (args.first(), args.get(1)) else {
            return Err(usage.into());
        };
        let setting = SETTINGS
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| *s)
            .ok_or_else(|| format!("unknown setting \"{name}\"; {usage}"))?;
        let rule = Rule {
            setting,
            value: parse(setting, value)?,
            device: args.get(2).cloned(),
        };
        let inputs = &mut self.inputs;
        if inputs.config.is_none() {
            return Err("river_libinput_config_v1 not available".into());
        }
        // a newer rule for the same setting and device replaces the old one
        inputs
            .rules
            .retain(|r| !(r.setting == rule.setting && r.device == rule.device));
        inputs.rules.push(rule.clone());
        for d in &inputs.devices {
            inputs.apply(d, &rule, &self.qh);
        }
        Ok(())
    }
}

impl Dispatch<RiverLibinputConfigV1, ()> for Wm {
    fn event(
        state: &mut Self,
        _: &RiverLibinputConfigV1,
        event: river_libinput_config_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let river_libinput_config_v1::Event::LibinputDevice { id } = event {
            state.inputs.devices.push(Device {
                obj: id,
                input: None,
                state: BTreeMap::new(),
                supported: Vec::new(),
            });
        }
    }

    wayland_client::event_created_child!(Wm, RiverLibinputConfigV1, [
        river_libinput_config_v1::EVT_LIBINPUT_DEVICE_OPCODE => (RiverLibinputDeviceV1, ()),
    ]);
}

impl Dispatch<RiverLibinputDeviceV1, ()> for Wm {
    fn event(
        state: &mut Self,
        proxy: &RiverLibinputDeviceV1,
        event: dev::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use dev::Event;
        let inputs = &mut state.inputs;
        let Some(i) = inputs.devices.iter().position(|d| d.obj == *proxy) else {
            return;
        };
        let d = &mut inputs.devices[i];
        let mut support = |key: &'static str, yes: bool| {
            if yes && !d.supported.contains(&key) {
                d.supported.push(key);
            }
        };
        match event {
            Event::Removed => {
                let d = inputs.devices.remove(i);
                d.obj.destroy();
            }
            Event::InputDevice { device } => {
                d.input = Some(device.id());
                inputs.apply_all(i, qh);
            }
            Event::TapSupport { finger_count } => {
                support("tap", finger_count > 0);
                support("drag", finger_count > 0);
            }
            Event::DwtSupport { supported } => support("dwt", supported != 0),
            Event::NaturalScrollSupport { supported } => support("natural_scroll", supported != 0),
            Event::LeftHandedSupport { supported } => support("left_handed", supported != 0),
            Event::MiddleEmulationSupport { supported } => {
                support("middle_emulation", supported != 0)
            }
            Event::AccelProfilesSupport { profiles } => {
                let any = raw(profiles) != 0;
                support("accel_profile", any);
                support("accel_speed", any);
            }
            Event::TapCurrent { state } => {
                d.state.insert("tap", on_off(raw(state) != 0));
            }
            Event::DragCurrent { state } => {
                d.state.insert("drag", on_off(raw(state) != 0));
            }
            Event::DwtCurrent { state } => {
                d.state.insert("dwt", on_off(raw(state) != 0));
            }
            Event::NaturalScrollCurrent { state } => {
                d.state.insert("natural_scroll", on_off(raw(state) != 0));
            }
            Event::LeftHandedCurrent { state } => {
                d.state.insert("left_handed", on_off(raw(state) != 0));
            }
            Event::MiddleEmulationCurrent { state } => {
                d.state.insert("middle_emulation", on_off(raw(state) != 0));
            }
            Event::AccelProfileCurrent { profile } => {
                let name = match profile {
                    WEnum::Value(AccelProfile::Flat) => "flat",
                    WEnum::Value(AccelProfile::Adaptive) => "adaptive",
                    WEnum::Value(AccelProfile::Custom) => "custom",
                    _ => "none",
                };
                d.state.insert("accel_profile", name.to_string());
            }
            Event::AccelSpeedCurrent { speed } => {
                if let Ok(bytes) = <[u8; 8]>::try_from(speed.as_slice()) {
                    d.state
                        .insert("accel_speed", format!("{:.2}", f64::from_ne_bytes(bytes)));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<RiverInputDeviceV1, ()> for Wm {
    fn event(
        state: &mut Self,
        proxy: &RiverInputDeviceV1,
        event: river_input_device_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            river_input_device_v1::Event::Name { name } => {
                let id = proxy.id();
                state.inputs.names.insert(id.clone(), name);
                // a libinput device may have referred to this one already
                for i in 0..state.inputs.devices.len() {
                    if state.inputs.devices[i].input.as_ref() == Some(&id) {
                        state.inputs.apply_all(i, qh);
                    }
                }
            }
            river_input_device_v1::Event::Removed => {
                state.inputs.names.remove(&proxy.id());
                proxy.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<RiverLibinputResultV1, (String, Setting)> for Wm {
    fn event(
        _: &mut Self,
        _: &RiverLibinputResultV1,
        event: river_libinput_result_v1::Event,
        (device, setting): &(String, Setting),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // unsupported is expected: rules without a device name reach every
        // device, keyboards included
        if let river_libinput_result_v1::Event::Invalid = event {
            eprintln!("hrwm: input {setting:?} was rejected by {device}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_values() {
        assert_eq!(parse(Setting::Tap, "on").unwrap(), Value::On(true));
        assert_eq!(parse(Setting::Dwt, "0").unwrap(), Value::On(false));
        assert!(parse(Setting::Tap, "maybe").is_err());
        assert_eq!(
            parse(Setting::AccelSpeed, "-0.5").unwrap(),
            Value::Speed(-0.5)
        );
        assert!(parse(Setting::AccelSpeed, "2").is_err());
        assert_eq!(
            parse(Setting::AccelProfile, "flat").unwrap(),
            Value::Profile(AccelProfile::Flat)
        );
    }

    #[test]
    fn device_filter_is_case_insensitive_substring() {
        let r = Rule {
            setting: Setting::Tap,
            value: Value::On(true),
            device: Some("touchpad".into()),
        };
        assert!(r.matches("ETPS/2 Elantech Touchpad"));
        assert!(!r.matches("ETPS/2 Elantech TrackPoint"));
    }
}
