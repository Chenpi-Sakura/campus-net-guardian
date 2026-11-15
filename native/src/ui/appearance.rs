//! 解析四套独立配色并校验可读性，不读取系统或用户文件。

use crate::config::{Appearance, ThemeMode, ThemeStyle};
use anyhow::{Result, bail};
use iced::{Color, Theme, theme::Palette, theme::palette};

#[derive(Clone, Copy)]
struct Colors {
    accent: Color,
    background: Color,
    surface: Color,
    text: Color,
}

/// 按用户选择解析实际明暗模式；仅跟随系统模式使用传入的系统值。
pub(super) fn is_dark(appearance: &Appearance, system_dark: bool) -> bool {
    match appearance.mode {
        ThemeMode::System => system_dark,
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
    }
}

fn defaults(style: ThemeStyle, dark: bool) -> Colors {
    let (accent, background, surface, text) = match (style, dark) {
        (ThemeStyle::Claude, false) => (0xBA5B3C, 0xFAF9F5, 0xFFFFFF, 0x141413),
        (ThemeStyle::Claude, true) => (0xD97757, 0x262624, 0x30302E, 0xFAF9F5),
        (ThemeStyle::ChatGpt, false) => (0x0D0D0D, 0xFFFFFF, 0xF9F9F9, 0x0D0D0D),
        (ThemeStyle::ChatGpt, true) => (0xEDEDED, 0x212121, 0x303030, 0xEDEDED),
    };
    Colors {
        accent: rgb(accent),
        background: rgb(background),
        surface: rgb(surface),
        text: rgb(text),
    }
}

fn rgb(value: u32) -> Color {
    Color::from_rgb8((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn parse(value: &str) -> Option<Color> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().map(rgb)
}

fn resolve(appearance: &Appearance, style: ThemeStyle, dark: bool) -> Colors {
    let preset = match style {
        ThemeStyle::Claude => &appearance.claude,
        ThemeStyle::ChatGpt => &appearance.chatgpt,
    };
    let overrides = if dark { &preset.dark } else { &preset.light };
    let default = defaults(style, dark);
    let color =
        |value: &Option<String>, fallback| value.as_deref().and_then(parse).unwrap_or(fallback);
    Colors {
        accent: color(&overrides.accent_color, default.accent),
        background: color(&overrides.background_color, default.background),
        surface: color(&overrides.surface_color, default.surface),
        text: color(&overrides.text_color, default.text),
    }
}

fn to_hex(color: Color) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        (color.r * 255.0).round() as u8,
        (color.g * 255.0).round() as u8,
        (color.b * 255.0).round() as u8,
    )
}

/// 返回当前配色的有效颜色文本，依次为强调色、背景色、表面色、文字色。
pub(super) fn fields(appearance: &Appearance, system_dark: bool) -> [String; 4] {
    let colors = resolve(
        appearance,
        appearance.style,
        is_dark(appearance, system_dark),
    );
    [
        colors.accent,
        colors.background,
        colors.surface,
        colors.text,
    ]
    .map(to_hex)
}

/// 校验全部四套配色的语法和正文可读性，包括当前未启用的预设。
pub(super) fn validate(appearance: &Appearance) -> Result<()> {
    appearance.validate()?;
    for &style in ThemeStyle::ALL {
        for dark in [false, true] {
            let colors = resolve(appearance, style, dark);
            for (name, background) in [("背景", colors.background), ("表面", colors.surface)] {
                if colors.text.relative_contrast(background) < 4.5 {
                    let mode = if dark { "深色" } else { "浅色" };
                    bail!("{style} {mode}的文字与{name}颜色过于接近，请调整文字色或{name}色");
                }
            }
        }
    }
    Ok(())
}

fn raised(surface: Color, text: Color) -> Color {
    Color::from_rgb(
        surface.r + (text.r - surface.r) * 0.045,
        surface.g + (text.g - surface.g) * 0.045,
        surface.b + (text.b - surface.b) * 0.045,
    )
}

/// 根据预设与覆盖项创建 Iced 主题；无效的临时颜色输入使用对应预设默认值。
pub(super) fn theme(appearance: &Appearance, system_dark: bool) -> Theme {
    let dark = is_dark(appearance, system_dark);
    let colors = resolve(appearance, appearance.style, dark);
    let palette = Palette {
        background: colors.background,
        text: colors.text,
        primary: colors.accent,
        success: if dark { rgb(0x82AA89) } else { rgb(0x2F7040) },
        warning: if dark { rgb(0xCFA26F) } else { rgb(0x885A22) },
        danger: if dark { rgb(0xD6917F) } else { rgb(0x9E4330) },
    };
    Theme::custom_with_fn(
        format!(
            "{} {}",
            appearance.style,
            if dark { "深色" } else { "浅色" }
        ),
        palette,
        move |palette| {
            let mut extended = palette::Extended::generate(palette);
            // Iced Pair::new 会为 6:1 阈值调整文字；正文已按 4.5:1 校验，保留用户选择。
            extended.background.base = palette::Pair {
                color: colors.background,
                text: colors.text,
            };
            extended.background.weak = palette::Pair {
                color: colors.surface,
                text: colors.text,
            };
            extended.background.strong = palette::Pair {
                color: raised(colors.surface, colors.text),
                text: colors.text,
            };
            extended
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ColorOverrides;

    fn set_colors(
        appearance: &mut Appearance,
        style: ThemeStyle,
        dark: bool,
        colors: ColorOverrides,
    ) {
        let preset = match style {
            ThemeStyle::Claude => &mut appearance.claude,
            ThemeStyle::ChatGpt => &mut appearance.chatgpt,
        };
        if dark {
            preset.dark = colors;
        } else {
            preset.light = colors;
        }
    }

    #[test]
    fn defaults_are_readable_and_preserve_each_design_palette() {
        let mut appearance = Appearance::default();
        validate(&appearance).unwrap();
        for (style, dark, expected) in [
            (
                ThemeStyle::Claude,
                false,
                ["#BA5B3C", "#FAF9F5", "#FFFFFF", "#141413"],
            ),
            (
                ThemeStyle::Claude,
                true,
                ["#D97757", "#262624", "#30302E", "#FAF9F5"],
            ),
            (
                ThemeStyle::ChatGpt,
                false,
                ["#0D0D0D", "#FFFFFF", "#F9F9F9", "#0D0D0D"],
            ),
            (
                ThemeStyle::ChatGpt,
                true,
                ["#EDEDED", "#212121", "#303030", "#EDEDED"],
            ),
        ] {
            appearance.style = style;
            assert_eq!(fields(&appearance, dark), expected.map(String::from));
            let theme = theme(&appearance, dark);
            assert_eq!(to_hex(theme.palette().primary), expected[0]);
            assert_eq!(to_hex(theme.palette().background), expected[1]);
            assert_eq!(
                to_hex(theme.extended_palette().background.weak.color),
                expected[2]
            );
            assert_eq!(
                to_hex(theme.extended_palette().background.weak.text),
                expected[3]
            );
        }
    }

    #[test]
    fn explicit_modes_override_system_value() {
        let mut appearance = Appearance::default();
        assert!(!is_dark(&appearance, false));
        assert!(is_dark(&appearance, true));
        appearance.mode = ThemeMode::Light;
        assert!(!is_dark(&appearance, true));
        appearance.mode = ThemeMode::Dark;
        assert!(is_dark(&appearance, false));
    }

    #[test]
    fn custom_palettes_remain_independent_across_style_and_mode() {
        let mut appearance = Appearance::default();
        appearance.claude.dark = ColorOverrides {
            accent_color: Some("#aabbcc".into()),
            background_color: Some("#000000".into()),
            surface_color: Some("#111111".into()),
            text_color: Some("#FFFFFF".into()),
        };
        appearance.chatgpt.light.accent_color = Some("#445566".into());
        validate(&appearance).unwrap();
        assert_eq!(
            fields(&appearance, true),
            ["#AABBCC", "#000000", "#111111", "#FFFFFF"].map(String::from)
        );
        assert_eq!(fields(&appearance, false)[1], "#FAF9F5");
        appearance.style = ThemeStyle::ChatGpt;
        assert_eq!(fields(&appearance, false)[0], "#445566");
        assert_eq!(fields(&appearance, true)[0], "#EDEDED");
    }

    #[test]
    fn validation_checks_inactive_palettes_and_both_text_surfaces() {
        for &style in ThemeStyle::ALL {
            for dark in [false, true] {
                for surface_only in [false, true] {
                    let mut appearance = Appearance::default();
                    let colors = if surface_only {
                        ColorOverrides {
                            background_color: Some("#FFFFFF".into()),
                            surface_color: Some("#000000".into()),
                            text_color: Some("#000000".into()),
                            ..Default::default()
                        }
                    } else {
                        ColorOverrides {
                            background_color: Some("#FFFFFF".into()),
                            surface_color: Some("#000000".into()),
                            text_color: Some("#FFFFFF".into()),
                            ..Default::default()
                        }
                    };
                    set_colors(&mut appearance, style, dark, colors);
                    assert!(validate(&appearance).is_err());
                }
            }
        }
    }

    #[test]
    fn contrast_threshold_is_four_point_five_and_accent_is_not_rejected() {
        let mut appearance = Appearance::default();
        appearance.claude.light = ColorOverrides {
            accent_color: Some("#FFFFFF".into()),
            background_color: Some("#FFFFFF".into()),
            surface_color: Some("#FFFFFF".into()),
            text_color: Some("#767676".into()),
        };
        validate(&appearance).unwrap();
        let theme = theme(&appearance, false);
        assert_eq!(
            to_hex(theme.extended_palette().background.weak.text),
            "#767676"
        );
        let primary = theme.extended_palette().primary.base;
        assert!(primary.text.relative_contrast(primary.color) >= 4.5);
        appearance.claude.light.text_color = Some("#777777".into());
        assert!(validate(&appearance).is_err());
    }

    #[test]
    fn malformed_color_is_rejected_before_contrast_validation() {
        let mut appearance = Appearance::default();
        appearance.chatgpt.dark.accent_color = Some("#12GG56".into());
        assert!(validate(&appearance).is_err());
    }
}
