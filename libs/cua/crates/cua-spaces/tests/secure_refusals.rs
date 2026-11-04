//! What Secure Spaces refuses, with the exact words, and that the refusals
//! only apply while the experiment is on. One test, because the experiment
//! switch is process-wide.

use cua_spaces::Error;
use cua_spaces::network::{
    NetworkPreset, NetworkSettings, parse_network, refuse_exit_by_agent, require_secure_spaces,
    with_via,
};
use cua_vmm::experiments::{Gate, set_enabled};

fn message(e: Error) -> String {
    e.to_string()
        .trim_start_matches("invalid argument: ")
        .to_string()
}

#[test]
fn the_refusals_say_what_to_do_and_apply_only_when_the_experiment_is_on() {
    // An agent may not choose this computer or a proxy as a Space's exit.
    let current = NetworkSettings::internet();
    for word in ["this-computer", "socks5://proxy.example:1080"] {
        let asked = with_via(NetworkSettings::internet(), word).unwrap();
        let e = refuse_exit_by_agent(&asked, Some(&current)).unwrap_err();
        assert_eq!(
            message(e),
            "exiting through this computer or a proxy is the user's to choose: ask them to run \
             `cua network set <space> --via ...` (or use the app)",
            "{word}"
        );
    }
    // Keeping the exit it has is not a change.
    let kept = with_via(NetworkSettings::internet(), "this-computer").unwrap();
    assert!(refuse_exit_by_agent(&kept, Some(&kept)).is_ok());
    // A Space of its tree an agent may pick.
    let space = with_via(NetworkSettings::internet(), "space:gateway").unwrap();
    assert!(refuse_exit_by_agent(&space, Some(&current)).is_ok());

    // Without the experiment, the new settings are refused with how to turn it on.
    set_enabled(Gate::SecureSpaces, Some(false));
    let off = Gate::SecureSpaces.off_message();
    let by_name = parse_network("internet", &["pypi.org".into()], &[]).unwrap();
    for (what, settings) in [
        ("a name rule", by_name),
        (
            "an exit",
            with_via(NetworkSettings::internet(), "this-computer").unwrap(),
        ),
        ("lan", NetworkSettings::preset(NetworkPreset::Lan)),
    ] {
        let e = require_secure_spaces(Some(&settings)).unwrap_err();
        assert_eq!(message(e), off, "{what}");
    }
    // The old lists and `none` still work.
    assert!(require_secure_spaces(Some(&NetworkSettings::internet())).is_ok());
    assert!(require_secure_spaces(Some(&NetworkSettings::none())).is_ok());
    // The hotspot call keeps its old shape without the experiment.
    assert!(cua_spaces::hotspot::require_confirmed(None).is_ok());

    // With it, the same are accepted, and a hotspot start needs the user's yes.
    set_enabled(Gate::SecureSpaces, Some(true));
    let lan = NetworkSettings::preset(NetworkPreset::Lan);
    assert!(require_secure_spaces(Some(&lan)).is_ok());
    let e = cua_spaces::hotspot::require_confirmed(None).unwrap_err();
    assert_eq!(
        message(e),
        "hotspot_start sends the Space's internet traffic out through this computer's network \
         (its own rules still apply: the internet only unless they allow more). Ask the \
         user, and call again with confirm=true only if they say yes."
    );
    assert!(cua_spaces::hotspot::require_confirmed(Some(false)).is_err());
    assert!(cua_spaces::hotspot::require_confirmed(Some(true)).is_ok());
    set_enabled(Gate::SecureSpaces, None);
}
