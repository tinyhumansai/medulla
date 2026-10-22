//! Tests for the theme module.

use super::*;
use medulla::config::ResourcesThemeConfig;

#[test]
fn parses_named_colors_case_insensitively() {
    assert_eq!(parse_color("cyan"), Some(Color::Cyan));
    assert_eq!(parse_color("CYAN"), Some(Color::Cyan));
    assert_eq!(parse_color("  LightBlue "), Some(Color::LightBlue));
    assert_eq!(parse_color("grey"), Some(Color::Gray));
}

#[test]
fn resource_theme_has_btop_cpu_gradient_defaults() {
    let gradient = Theme::default().resources.cpu;
    assert_eq!(gradient.at(0), Color::Rgb(0x77, 0xca, 0x9b));
    assert_eq!(gradient.at(50), Color::Rgb(0xcb, 0xc0, 0x6c));
    assert_eq!(gradient.at(100), Color::Rgb(0xdc, 0x4c, 0x4c));
}

#[test]
fn gradients_support_two_colors_and_solid_fills() {
    let two = Gradient::new(Color::Rgb(0, 10, 20), None, Some(Color::Rgb(100, 110, 120)));
    assert_eq!(two.at(0), Color::Rgb(0, 10, 20));
    assert_eq!(two.at(50), Color::Rgb(50, 60, 70));
    assert_eq!(two.at(100), Color::Rgb(100, 110, 120));

    let solid = Gradient::new(Color::Rgb(9, 8, 7), None, None);
    assert_eq!(solid.at(0), Color::Rgb(9, 8, 7));
    assert_eq!(solid.at(50), Color::Rgb(9, 8, 7));
    assert_eq!(solid.at(100), Color::Rgb(9, 8, 7));
}

#[test]
fn truecolor_fallback_matches_btop_256_color_indices() {
    assert_eq!(truecolor_to_256(Color::Rgb(255, 0, 0)), Color::Indexed(196));
    assert_eq!(
        truecolor_to_256(Color::Rgb(128, 128, 128)),
        Color::Indexed(244)
    );
    assert_eq!(
        truecolor_to_256(Color::Rgb(0, 255, 255)),
        Color::Indexed(51)
    );
}

#[test]
fn terminal_depth_prefers_colorterm_and_uses_ansi_colors_for_tty() {
    assert_eq!(
        ColorDepth::detect_from(Some("24bit"), None),
        ColorDepth::TrueColor
    );
    assert_eq!(
        ColorDepth::detect_from(None, Some("xterm-256color")),
        ColorDepth::Color256
    );
    assert_eq!(
        ColorDepth::detect_from(None, Some("vt100")),
        ColorDepth::Color16
    );
    assert_eq!(truecolor_to_16(Color::Rgb(255, 0, 0)), Color::LightRed);
}

#[test]
fn named_color_gradients_interpolate_between_their_rgb_values() {
    let gradient = Gradient::new(Color::Red, None, Some(Color::Blue));
    assert_eq!(gradient.at(0), Color::Red);
    assert_eq!(gradient.at(100), Color::Blue);
    assert_eq!(gradient.at(50), Color::Rgb(103, 0, 119));
}

#[test]
fn resource_config_override_keeps_other_btop_defaults() {
    let theme = Theme::from_config(&ThemeConfig {
        resources: Some(ResourcesThemeConfig {
            cpu_box: Some("#010203".into()),
            ..ResourcesThemeConfig::default()
        }),
        ..ThemeConfig::default()
    });
    assert_eq!(theme.resources.cpu_box, Color::Rgb(1, 2, 3));
    assert_eq!(theme.resources.mem_box, Color::Rgb(0x6c, 0x6c, 0x4b));
}

#[test]
fn empty_gradient_controls_select_two_color_and_solid_forms() {
    let theme = Theme::from_config(&ThemeConfig {
        resources: Some(ResourcesThemeConfig {
            cpu_start: Some("#000000".into()),
            cpu_mid: Some(String::new()),
            cpu_end: Some("#ffffff".into()),
            temp_start: Some("#123456".into()),
            temp_mid: Some(String::new()),
            temp_end: Some(String::new()),
            ..ResourcesThemeConfig::default()
        }),
        ..ThemeConfig::default()
    });
    assert_eq!(theme.resources.cpu.at(50), Color::Rgb(127, 127, 127));
    assert_eq!(theme.resources.temp.at(100), Color::Rgb(0x12, 0x34, 0x56));
}

#[test]
fn parses_hex_colors() {
    assert_eq!(parse_color("#ff8800"), Some(Color::Rgb(255, 136, 0)));
    assert_eq!(parse_color("#000000"), Some(Color::Rgb(0, 0, 0)));
}

#[test]
fn rejects_garbage() {
    assert_eq!(parse_color("notacolor"), None);
    assert_eq!(parse_color("#fff"), None);
    assert_eq!(parse_color("#gggggg"), None);
    assert_eq!(parse_color("#ff88000"), None);
}

#[test]
fn from_config_falls_back_per_field() {
    let cfg = ThemeConfig {
        primary: Some("#123456".into()),
        accent: Some("bogus".into()),
        selection_fg: None,
        dim_border: Some("blue".into()),
        attention: Some("lightyellow".into()),
        attention_blink: Some(false),
        attention_blink_seconds: None,
        ..ThemeConfig::default()
    };
    let t = Theme::from_config(&cfg);
    assert_eq!(t.primary, Color::Rgb(0x12, 0x34, 0x56));
    assert_eq!(t.accent, Theme::default().accent); // bogus → fallback
    assert_eq!(t.selection_fg, Theme::default().selection_fg); // none → fallback
    assert_eq!(t.dim_border, Color::Blue);
    assert_eq!(t.attention, Color::LightYellow);
    assert!(!t.attention_blink);
}

#[test]
fn default_primary_is_red() {
    assert_eq!(Theme::default().primary, Color::Red);
    assert_eq!(Theme::default().selection_fg, Color::White);
}

#[test]
fn cycle_role_walks_palette_and_wraps() {
    let mut t = Theme::default();
    assert_eq!(t.role(0), Color::Red);
    t.cycle_role(0, true);
    assert_eq!(t.role(0), Color::Cyan); // PALETTE[1]
    t.cycle_role(0, false);
    assert_eq!(t.role(0), Color::Red);
}

#[test]
fn cycle_role_from_custom_steps_into_palette() {
    let mut t = Theme {
        primary: Color::Rgb(1, 2, 3),
        ..Theme::default()
    };
    t.cycle_role(0, true);
    assert_eq!(t.role(0), PALETTE[0]);
}

#[test]
fn color_to_string_round_trips_named_and_hex() {
    for c in PALETTE {
        assert_eq!(parse_color(&color_to_string(c)), Some(c));
    }
    let rgb = Color::Rgb(0xab, 0xcd, 0xef);
    assert_eq!(color_to_string(rgb), "#abcdef");
    assert_eq!(parse_color("#abcdef"), Some(rgb));
}

#[test]
fn color_to_string_covers_every_named_variant() {
    for (color, name) in [
        (Color::Black, "black"),
        (Color::Gray, "gray"),
        (Color::DarkGray, "darkgray"),
        (Color::White, "white"),
        (Color::LightRed, "lightred"),
        (Color::LightGreen, "lightgreen"),
        (Color::LightYellow, "lightyellow"),
        (Color::LightBlue, "lightblue"),
        (Color::LightMagenta, "lightmagenta"),
        (Color::LightCyan, "lightcyan"),
        (Color::Reset, "reset"),
    ] {
        assert_eq!(color_to_string(color), name);
    }
    // Indexed colours fall through to the `indexed<N>` form.
    assert_eq!(color_to_string(Color::Indexed(42)), "indexed42");
}

#[test]
fn cycle_role_covers_all_roles_and_custom_backward() {
    let mut t = Theme::default();
    // Every role index round-trips through set_role.
    for idx in 0..THEME_ROLES.len() {
        let before = t.role(idx);
        t.cycle_role(idx, true);
        assert_ne!(t.role(idx), before, "role {idx} advanced");
    }
    // A custom value stepped backward lands on the last palette entry.
    let mut custom = Theme {
        selection_fg: Color::Rgb(9, 9, 9),
        ..Theme::default()
    };
    custom.cycle_role(2, false);
    assert_eq!(custom.role(2), PALETTE[PALETTE.len() - 1]);
}

#[test]
fn persist_theme_writes_and_reloads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("config.toml");
    let theme = Theme {
        primary: Color::Rgb(0x10, 0x20, 0x30),
        accent: Color::Green,
        ..Theme::default()
    };
    persist_theme(&path, &theme).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[theme]"), "section header: {text}");
    assert!(text.contains("primary = \"#102030\""), "primary: {text}");
    assert!(text.contains("accent = \"green\""), "accent: {text}");
}

#[test]
fn persist_theme_preserves_unrelated_sections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "stateDir = \"/tmp/state\"\n\n[backend]\nbaseUrl = \"https://example.test\"\n\n[medulla]\nmaxPasses = 8\n",
    )
    .unwrap();

    persist_theme(&path, &Theme::default()).unwrap();

    let reparsed: toml::Table = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        reparsed["backend"]["baseUrl"].as_str(),
        Some("https://example.test")
    );
    assert_eq!(reparsed["medulla"]["maxPasses"].as_integer(), Some(8));
    assert_eq!(reparsed["stateDir"].as_str(), Some("/tmp/state"));
    assert_eq!(reparsed["theme"]["primary"].as_str(), Some("red"));
    assert_eq!(reparsed["theme"]["attention"].as_str(), Some("yellow"));
    assert_eq!(reparsed["theme"]["attentionBlink"].as_bool(), Some(true));
}

/// A pulse configured in seconds becomes whole frames of the render clock, and
/// nonsense is clamped rather than dropped: a config that asks for something
/// unusable should still change the pulse in the direction it asked for.
#[test]
fn a_configured_blink_rate_is_read_in_seconds_and_clamped() {
    let with = |seconds: Option<f64>| {
        Theme::from_config(&ThemeConfig {
            attention_blink_seconds: seconds,
            ..Default::default()
        })
        .attention_blink_ms
    };

    assert_eq!(with(Some(2.5)), 2_500);
    assert_eq!(with(None), Theme::default().attention_blink_ms);
    assert_eq!(with(Some(0.0)), 200, "clamped up to the shortest pulse");
    assert_eq!(with(Some(600.0)), 10_000, "clamped down to the longest");
    assert_eq!(
        with(Some(f64::NAN)),
        Theme::default().attention_blink_ms,
        "a value that asks for nothing takes the default"
    );
}

/// The pulse alternates on the frame counter rather than delegating to
/// `SLOW_BLINK`, which most terminals ignore.
#[test]
fn the_pulse_alternates_at_the_configured_rate() {
    // A one-second pulse rounds each 500ms phase to six 90ms frames.
    let theme = Theme {
        attention_blink_ms: 1_000,
        ..Default::default()
    };
    let half = (500 + FRAME_MS as usize / 2) / FRAME_MS as usize;

    assert!(theme.attention_bright(0));
    assert!(theme.attention_bright(half - 1));
    assert!(!theme.attention_bright(half));
    assert!(!theme.attention_bright(half * 2 - 1));
    assert!(theme.attention_bright(half * 2));
}

/// Switching blinking off must not leave the cue stuck in its dim phase.
#[test]
fn a_theme_that_does_not_blink_is_always_bright() {
    let theme = Theme {
        attention_blink: false,
        ..Default::default()
    };

    assert!((0..40).all(|frame| theme.attention_bright(frame)));
}

/// A period shorter than one frame still has to alternate rather than divide to
/// zero and strobe on every tick.
#[test]
fn a_period_below_the_frame_rate_still_holds_each_phase_a_frame() {
    let theme = Theme {
        attention_blink_ms: MIN_BLINK_MS,
        ..Default::default()
    };

    assert!(theme.attention_bright(0));
    assert!(!theme.attention_bright(1));
    assert!(theme.attention_bright(2));
}
