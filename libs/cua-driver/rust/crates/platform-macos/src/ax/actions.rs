#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomAction {
    pub name: String,
    pub raw: String,
}

#[derive(Debug, Default)]
pub struct ElementActions {
    pub standard: Vec<String>,
    pub custom: Vec<CustomAction>,
}

impl ElementActions {
    pub fn is_empty(&self) -> bool {
        self.standard.is_empty() && self.custom.is_empty()
    }
}

pub fn split(advertised: Vec<String>) -> ElementActions {
    let mut actions = ElementActions::default();
    for raw in advertised {
        if raw.starts_with("AX") {
            actions.standard.push(raw);
            continue;
        }
        let name = custom_action_name(&raw);
        if name.is_empty() || actions.custom.iter().any(|action| action.raw == raw) {
            continue;
        }
        actions.custom.push(CustomAction { name, raw });
    }
    actions
}

fn custom_action_name(raw: &str) -> String {
    let first_line = raw.lines().next().unwrap_or_default();
    first_line
        .strip_prefix("Name:")
        .unwrap_or(first_line)
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advertised(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_custom_action_is_read_out_of_its_envelope_and_kept_invocable() {
        let actions = split(advertised(&[
            "AXShowMenu",
            "Name:Move Down\nTarget:0x0\nSelector:(null)",
        ]));
        assert_eq!(actions.standard, vec!["AXShowMenu".to_owned()]);
        assert_eq!(
            actions.custom,
            vec![CustomAction {
                name: "Move Down".to_owned(),
                raw: "Name:Move Down\nTarget:0x0\nSelector:(null)".to_owned(),
            }]
        );
    }

    #[test]
    fn a_cell_that_lists_the_same_custom_action_twice_publishes_it_once() {
        let actions = split(advertised(&[
            "Name:Pin List\nTarget:0x0\nSelector:(null)",
            "Name:Details\nTarget:0x0\nSelector:(null)",
            "Name:Pin List\nTarget:0x0\nSelector:(null)",
        ]));
        assert_eq!(
            actions
                .custom
                .iter()
                .map(|action| action.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Pin List", "Details"]
        );
    }

    #[test]
    fn a_nameless_envelope_is_not_published_as_an_action() {
        let actions = split(advertised(&["Name:\nTarget:0x0", "  ", ""]));
        assert!(actions.custom.is_empty());
        assert!(actions.is_empty());
    }

    #[test]
    fn an_unenveloped_name_is_still_custom_and_keeps_its_wire_form() {
        let actions = split(advertised(&["Flag"]));
        assert_eq!(
            actions.custom,
            vec![CustomAction {
                name: "Flag".to_owned(),
                raw: "Flag".to_owned(),
            }]
        );
        assert!(actions.standard.is_empty());
    }
}
