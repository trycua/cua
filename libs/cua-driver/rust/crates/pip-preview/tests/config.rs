use pip_preview::PipConfig;

#[test]
fn preview_is_disabled_by_default() {
    assert!(!PipConfig::default().enabled);
    assert!(!PipConfig::parse(&[]).enabled);
}

#[test]
fn either_explicit_opt_in_enables_the_preview() {
    for flag in ["--experimental-pip", "--pip"] {
        assert!(PipConfig::parse(&["serve".into(), flag.into()]).enabled);
    }
}

#[test]
fn geometry_alone_does_not_enable_the_preview() {
    let config = PipConfig::parse(&["--experimental-pip-geometry".into(), "640x400+20+30".into()]);
    assert!(!config.enabled);
    assert_eq!(config.geometry.width, 640);
    assert_eq!(config.geometry.height, 400);
    assert_eq!(config.geometry.x, Some(20));
    assert_eq!(config.geometry.y, Some(30));
}
