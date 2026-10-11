//! herbstluftwm-style window rules (`rule`, `unrule`).
//!
//! Wayland windows only have an app_id and a title, so `class` and
//! `instance` both match the app_id, and `windowtype` is
//! `_NET_WM_WINDOW_TYPE_DIALOG` for windows with a parent and
//! `_NET_WM_WINDOW_TYPE_NORMAL` otherwise.

use regex::Regex;

#[derive(Debug)]
enum Matcher {
    Equals(String),
    /// the compiled regex and the pattern as the user wrote it
    Regex(Regex, String),
}

#[derive(Debug)]
struct Condition {
    property: String,
    matcher: Matcher,
    negated: bool,
}

#[derive(Debug)]
pub struct Rule {
    label: String,
    conditions: Vec<Condition>,
    consequences: Vec<(String, String)>,
    once: bool,
}

/// What a window looks like to the rules.
pub struct Props<'a> {
    pub app_id: &'a str,
    pub title: &'a str,
    pub has_parent: bool,
    pub pid: i32,
}

/// The merged consequences of all matching rules.
#[derive(Debug, Default, Clone)]
pub struct Outcome {
    pub tag: Option<String>,
    pub monitor: Option<String>,
    pub focus: Option<bool>,
    pub switchtag: Option<bool>,
    pub manage: Option<bool>,
    pub floating: Option<bool>,
    pub pseudotile: Option<bool>,
    pub fullscreen: Option<bool>,
}

const CONDITIONS: &[&str] = &[
    "class",
    "instance",
    "title",
    "pid",
    "windowtype",
    "windowrole",
    "maxage",
    "fixedsize",
];

fn on_off(v: &str) -> bool {
    matches!(v, "on" | "true" | "1")
}

#[derive(Default)]
pub struct Rules {
    rules: Vec<Rule>,
    next_label: u64,
}

impl Rules {
    /// `rule [[--]CONDITION=VALUE|~REGEX ...] [[--]CONSEQUENCE=VALUE ...]`
    pub fn add(&mut self, args: &[String]) -> Result<String, String> {
        let mut rule = Rule {
            label: String::new(),
            conditions: Vec::new(),
            consequences: Vec::new(),
            once: false,
        };
        let mut negate_next = false;
        let mut print_label = false;
        for arg in args {
            let arg = arg.strip_prefix("--").unwrap_or(arg);
            match arg {
                "not" | "!" => {
                    negate_next = true;
                    continue;
                }
                "once" => {
                    rule.once = true;
                    continue;
                }
                "printlabel" => {
                    print_label = true;
                    continue;
                }
                _ => {}
            }
            let split = arg
                .find(['=', '~'])
                .ok_or_else(|| format!("invalid rule argument \"{arg}\""))?;
            let (key, op, value) = (&arg[..split], &arg[split..split + 1], &arg[split + 1..]);
            if key == "label" {
                rule.label = value.to_string();
            } else if CONDITIONS.contains(&key) {
                let matcher = if op == "~" {
                    let re = Regex::new(&format!("^(?:{value})$"))
                        .map_err(|e| format!("invalid regex \"{value}\": {e}"))?;
                    Matcher::Regex(re, value.to_string())
                } else {
                    Matcher::Equals(value.to_string())
                };
                rule.conditions.push(Condition {
                    property: key.to_string(),
                    matcher,
                    negated: negate_next,
                });
            } else {
                rule.consequences.push((key.to_string(), value.to_string()));
            }
            negate_next = false;
        }
        if rule.label.is_empty() {
            rule.label = self.next_label.to_string();
            self.next_label += 1;
        }
        let label = rule.label.clone();
        self.rules.push(rule);
        Ok(if print_label {
            format!("{label}\n")
        } else {
            String::new()
        })
    }

    /// `unrule LABEL|--all|-F`
    pub fn remove(&mut self, label: &str) -> Result<(), String> {
        if matches!(label, "--all" | "-F") {
            self.rules.clear();
            return Ok(());
        }
        let before = self.rules.len();
        self.rules.retain(|r| r.label != label);
        if self.rules.len() == before {
            return Err(format!("no rule with label \"{label}\""));
        }
        Ok(())
    }

    pub fn list(&self) -> String {
        let mut out = String::new();
        for r in &self.rules {
            out.push_str(&format!("label={}", r.label));
            for c in &r.conditions {
                let (op, v) = match &c.matcher {
                    Matcher::Equals(v) => ("=", v.clone()),
                    Matcher::Regex(_, v) => ("~", v.clone()),
                };
                let not = if c.negated { "not " } else { "" };
                out.push_str(&format!("\t{not}{}{op}{v}", c.property));
            }
            for (k, v) in &r.consequences {
                out.push_str(&format!("\t{k}={v}"));
            }
            out.push('\n');
        }
        out
    }

    pub fn apply(&mut self, p: &Props) -> Outcome {
        let mut out = Outcome::default();
        let mut used_once = Vec::new();
        for (i, rule) in self.rules.iter().enumerate() {
            if !rule.conditions.iter().all(|c| condition_matches(c, p)) {
                continue;
            }
            for (k, v) in &rule.consequences {
                match k.as_str() {
                    "tag" => out.tag = Some(v.clone()),
                    "monitor" => out.monitor = Some(v.clone()),
                    "focus" => out.focus = Some(on_off(v)),
                    "switchtag" => out.switchtag = Some(on_off(v)),
                    "manage" => out.manage = Some(on_off(v)),
                    "floating" => out.floating = Some(on_off(v)),
                    "pseudotile" => out.pseudotile = Some(on_off(v)),
                    "fullscreen" => out.fullscreen = Some(on_off(v)),
                    _ => {}
                }
            }
            if rule.once {
                used_once.push(i);
            }
        }
        for i in used_once.into_iter().rev() {
            self.rules.remove(i);
        }
        out
    }
}

fn condition_matches(c: &Condition, p: &Props) -> bool {
    let value = match c.property.as_str() {
        "class" | "instance" => p.app_id.to_string(),
        "title" => p.title.to_string(),
        "pid" => p.pid.to_string(),
        "windowtype" => if p.has_parent {
            "_NET_WM_WINDOW_TYPE_DIALOG"
        } else {
            "_NET_WM_WINDOW_TYPE_NORMAL"
        }
        .to_string(),
        "windowrole" => String::new(),
        // a window is only ever checked when it appears
        "maxage" => return !c.negated,
        _ => return false,
    };
    let hit = match &c.matcher {
        Matcher::Equals(v) => *v == value,
        Matcher::Regex(re, _) => re.is_match(&value),
    };
    hit != c.negated
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn users_rules() {
        let mut r = Rules::default();
        r.add(&args(&["focus=on"])).unwrap();
        r.add(&args(&[
            "windowtype~_NET_WM_WINDOW_TYPE_(DIALOG|UTILITY|SPLASH)",
            "pseudotile=on",
        ]))
        .unwrap();
        r.add(&args(&["class=URxvt", "pseudotile=on"])).unwrap();
        let normal = Props {
            app_id: "foot",
            title: "x",
            has_parent: false,
            pid: 1,
        };
        let o = r.apply(&normal);
        assert_eq!(o.focus, Some(true));
        assert_eq!(o.pseudotile, None);
        let dialog = Props {
            app_id: "firefox",
            title: "Save",
            has_parent: true,
            pid: 1,
        };
        assert_eq!(r.apply(&dialog).pseudotile, Some(true));
        r.remove("-F").unwrap();
        assert_eq!(r.apply(&normal).focus, None);
    }

    #[test]
    fn once_and_negation() {
        let mut r = Rules::default();
        r.add(&args(&["once", "not", "class=foot", "tag=x"]))
            .unwrap();
        let foot = Props {
            app_id: "foot",
            title: "",
            has_parent: false,
            pid: 1,
        };
        let ff = Props {
            app_id: "firefox",
            title: "",
            has_parent: false,
            pid: 1,
        };
        assert_eq!(r.apply(&foot).tag, None);
        assert_eq!(r.apply(&ff).tag.as_deref(), Some("x"));
        assert_eq!(r.apply(&ff).tag, None);
    }
}
