//! 使用 Iced 标准控件呈现状态和同窗设置；暖中性色降低常驻工具的视觉干扰。

use super::{
    App, DisplayScaleChoice, IntervalChoice, Message, Page, RecoveryChoice, ServiceChoice,
    SettingsTab, ThresholdChoice, motion, scroll_edge, smooth_scroll,
};
use crate::config::{ThemeMode, ThemeStyle};
use crate::daemon::Status;
use iced::{
    Alignment, Background, Border, Color, Element, Fill, Font, Shadow, Theme, Vector, font,
    widget::{
        Column, Text, column, container, image, pick_list, responsive, row, rule, scrollable,
        space, text, text_input,
    },
};
use iced_anim::widget::button::{self, Button};
use std::time::{Duration, Instant};

const TWO_COLUMN_WIDTH: f32 = 680.0;
const CONTENT_MAX_WIDTH: f32 = 1000.0;
const COMPACT_WIDTH: f32 = 320.0;
const RUNNING_WIDE_WIDTH: f32 = 760.0;

#[derive(Clone, Copy)]
struct Colors {
    background: Color,
    surface: Color,
    raised: Color,
    border: Color,
    text: Color,
    muted: Color,
    accent: Color,
    accent_text: Color,
}

fn colors(theme: &Theme) -> Colors {
    let palette = theme.palette();
    let extended = theme.extended_palette();
    let muted = mix(palette.background, palette.text, 0.66);
    let muted = if muted.relative_contrast(palette.background) >= 4.5
        && muted.relative_contrast(extended.background.weak.color) >= 4.5
    {
        muted
    } else {
        palette.text
    };
    Colors {
        background: palette.background,
        surface: extended.background.weak.color,
        raised: extended.background.strong.color,
        border: mix(extended.background.weak.color, palette.text, 0.12),
        text: palette.text,
        muted,
        accent: palette.primary,
        accent_text: extended.primary.base.text,
    }
}

fn mix(base: Color, target: Color, amount: f32) -> Color {
    Color::from_rgba(
        base.r + (target.r - base.r) * amount,
        base.g + (target.g - base.g) * amount,
        base.b + (target.b - base.b) * amount,
        base.a + (target.a - base.a) * amount,
    )
}

fn label<'a>(value: impl ToString, size: f32) -> Text<'a> {
    text(value.to_string())
        .size(size)
        .shaping(text::Shaping::Advanced)
}

fn centered<'a>(value: impl ToString, size: f32) -> Text<'a> {
    label(value, size).width(Fill).align_x(Alignment::Center)
}

fn button<'a>(content: impl Into<Element<'a, Message>>) -> Button<'a, Message> {
    Button::new(content).animation(
        iced_anim::transition::Easing::EASE_OUT.with_duration(Duration::from_millis(120)),
    )
}

fn inset_content<'a>(
    content: impl Into<Element<'a, Message>>,
    padding: iced::Padding,
) -> Element<'a, Message> {
    container(container(content).width(Fill).max_width(CONTENT_MAX_WIDTH))
        .center_x(Fill)
        .padding(padding)
        .into()
}

fn title<'a>(value: impl ToString, size: f32) -> Text<'a> {
    label(value, size).font(Font {
        family: font::Family::Name("Microsoft YaHei UI"),
        weight: font::Weight::Semibold,
        ..Font::DEFAULT
    })
}

fn muted<'a>(value: impl ToString, size: f32) -> Text<'a> {
    label(value, size).style(|theme: &Theme| text::Style {
        color: Some(colors(theme).muted),
    })
}

fn line<'a>() -> Element<'a, Message> {
    rule::horizontal(1)
        .style(|theme: &Theme| rule::Style {
            color: colors(theme).border,
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: true,
        })
        .into()
}

fn surface(theme: &Theme) -> container::Style {
    let p = colors(theme);
    container::Style {
        text_color: Some(p.text),
        background: Some(p.surface.into()),
        border: Border {
            color: p.border,
            width: 1.0,
            radius: 16.0.into(),
        },
        ..Default::default()
    }
}

fn root_surface(theme: &Theme) -> container::Style {
    let p = colors(theme);
    container::Style {
        text_color: Some(p.text),
        background: Some(p.background.into()),
        ..Default::default()
    }
}

fn primary(theme: &Theme, status: button::Status) -> button::Style {
    let p = colors(theme);
    let color = match status {
        button::Status::Hovered => mix(p.accent, p.accent_text, 0.08),
        button::Status::Pressed => mix(p.accent, p.text, 0.12),
        button::Status::Disabled => p.raised,
        button::Status::Active => p.accent,
    };
    button::Style {
        background: Some(color.into()),
        text_color: if status == button::Status::Disabled {
            p.muted
        } else {
            p.accent_text
        },
        border: Border {
            radius: 10.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn secondary(theme: &Theme, status: button::Status) -> button::Style {
    let p = colors(theme);
    button::Style {
        background: Some(
            if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                p.raised
            } else {
                p.surface
            }
            .into(),
        ),
        text_color: if status == button::Status::Disabled {
            p.muted
        } else {
            p.text
        },
        border: Border {
            color: p.border,
            width: 1.0,
            radius: 10.0.into(),
        },
        ..Default::default()
    }
}

fn quiet(theme: &Theme, status: button::Status) -> button::Style {
    let p = colors(theme);
    button::Style {
        background: matches!(status, button::Status::Hovered | button::Status::Pressed)
            .then_some(p.raised.into()),
        text_color: if status == button::Status::Disabled {
            p.muted
        } else {
            p.text
        },
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn field(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let p = colors(theme);
    text_input::Style {
        background: Background::Color(p.surface),
        border: Border {
            color: if matches!(status, text_input::Status::Focused { .. }) {
                p.accent
            } else {
                p.border
            },
            width: 1.0,
            radius: 9.0.into(),
        },
        icon: p.muted,
        placeholder: p.muted,
        value: if status == text_input::Status::Disabled {
            p.muted
        } else {
            p.text
        },
        selection: Color {
            a: 0.25,
            ..p.accent
        },
    }
}

fn selection(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let p = colors(theme);
    pick_list::Style {
        text_color: p.text,
        placeholder_color: p.muted,
        handle_color: p.muted,
        background: p.surface.into(),
        border: Border {
            color: if matches!(status, pick_list::Status::Opened { .. }) {
                p.accent
            } else {
                p.border
            },
            width: 1.0,
            radius: if matches!(status, pick_list::Status::Opened { .. }) {
                if motion::popup_above() {
                    iced::border::Radius {
                        bottom_left: 9.0,
                        bottom_right: 9.0,
                        ..Default::default()
                    }
                } else {
                    iced::border::Radius {
                        top_left: 9.0,
                        top_right: 9.0,
                        ..Default::default()
                    }
                }
            } else {
                9.0.into()
            },
        },
    }
}

fn menu(theme: &Theme) -> iced::widget::overlay::menu::Style {
    let p = colors(theme);
    let selected = mix(p.surface, p.accent, 0.10);
    let dark = p.background.r + p.background.g + p.background.b < 1.5;
    iced::widget::overlay::menu::Style {
        background: p.surface.into(),
        border: Border {
            color: p.border,
            width: 1.0,
            radius: if motion::popup_above() {
                iced::border::Radius {
                    top_left: 11.0,
                    top_right: 11.0,
                    ..Default::default()
                }
            } else {
                iced::border::Radius {
                    bottom_left: 11.0,
                    bottom_right: 11.0,
                    ..Default::default()
                }
            },
        },
        text_color: p.text,
        selected_text_color: if p.text.relative_contrast(selected) >= 4.5 {
            p.text
        } else {
            iced::theme::palette::readable(selected, p.text)
        },
        selected_background: selected.into(),
        shadow: Shadow {
            color: Color {
                a: if dark { 0.16 } else { 0.09 },
                ..Color::BLACK
            },
            offset: Vector::new(0.0, 3.0),
            blur_radius: 8.0,
        },
    }
}

fn edge_scrollbar() -> scrollable::Scrollbar {
    scrollable::Scrollbar::new()
        .width(5)
        .scroller_width(5)
        .margin(3)
        .spacing(0)
}

fn scrollbar(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let p = colors(theme);
    let mut style = scrollable::default(theme, status);
    let opacity = match status {
        scrollable::Status::Dragged { .. } => 0.50,
        scrollable::Status::Hovered { .. } => 0.35,
        scrollable::Status::Active { .. } => 0.23,
    };
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: Color {
                a: opacity,
                ..p.text
            }
            .into(),
            border: Border {
                radius: 4.0.into(),
                ..Default::default()
            },
        },
    };
    style.vertical_rail = rail;
    style.horizontal_rail = rail;
    style
}

fn notice<'a>(message: impl ToString, error: bool) -> Element<'a, Message> {
    container(label(message, 12.0))
        .width(Fill)
        .padding([8, 10])
        .style(move |theme: &Theme| {
            let p = colors(theme);
            container::Style {
                background: Some(p.surface.into()),
                text_color: Some(if error {
                    let dark = p.background.r + p.background.g + p.background.b < 1.5;
                    if dark {
                        Color::from_rgb8(214, 145, 127)
                    } else {
                        Color::from_rgb8(158, 67, 48)
                    }
                } else {
                    p.muted
                }),
                border: Border {
                    color: p.border,
                    width: 1.0,
                    radius: 10.0.into(),
                },
                ..Default::default()
            }
        })
        .into()
}

/// 根据应用状态呈现同一窗口中的首页、设置页或退出确认。
pub(super) fn view(app: &App) -> Element<'_, Message> {
    let key = if app.exit_confirm {
        5
    } else {
        match app.page {
            Page::Home => 0,
            Page::Running => 1,
            Page::Settings => match app.settings_tab {
                SettingsTab::Global => 2,
                SettingsTab::Network => 3,
                SettingsTab::Running => 4,
            },
        }
    };
    let page = if app.exit_confirm {
        exit_confirmation(app)
    } else {
        match app.page {
            Page::Home => home(app),
            Page::Running => running(app),
            Page::Settings => settings(app),
        }
    };
    let content = container(page)
        .width(Fill)
        .height(Fill)
        .style(root_surface)
        .into();
    motion::page(key, app.theme(app.window), content)
}

fn home(app: &App) -> Element<'_, Message> {
    responsive(move |size| home_at_width(app, size.width)).into()
}

fn home_at_width(app: &App, width: f32) -> Element<'_, Message> {
    let wide = width >= TWO_COLUMN_WIDTH;
    let compact = width < COMPACT_WIDTH;
    let available_width = (width - 43.0).min(CONTENT_MAX_WIDTH);
    let status_width = if wide {
        (available_width - 20.0) / 2.0
    } else {
        available_width
    };
    let mut heading = column![title("CUIT Helper", 20.0).wrapping(text::Wrapping::Glyph)]
        .spacing(3)
        .width(Fill);
    if !compact {
        heading = heading.push(muted("保持连接，少一点打扰。", 11.0));
    }
    let header = row![
        image(app.logo.clone())
            .width(34)
            .height(34)
            .filter_method(image::FilterMethod::Linear)
            .content_fit(iced::ContentFit::Contain),
        heading
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let (state, badge) = match app.snapshot.status {
        Status::Online => ("连接正常", "已连接"),
        Status::Authenticating => ("正在连接…", "认证中"),
        Status::Fallback => ("正在使用备用网络", "备用"),
        Status::Paused => ("守护已暂停", "已暂停"),
        Status::Starting => ("正在准备…", "启动中"),
        Status::Stopped => ("守护已停止", "已停止"),
        Status::Offline if app.startup_error.is_some() => ("等待完成配置", "待配置"),
        Status::Offline => ("连接暂不可用", "离线"),
    };
    let status = app.snapshot.status;
    let startup_failed = app.startup_error.is_some();
    let status_color = move |theme: &Theme| {
        let p = colors(theme);
        let dark = p.background.r + p.background.g + p.background.b < 1.5;
        match status {
            Status::Online | Status::Authenticating | Status::Fallback | Status::Starting => {
                p.accent
            }
            Status::Paused | Status::Stopped => p.muted,
            Status::Offline if startup_failed && dark => Color::from_rgb8(207, 162, 111),
            Status::Offline if startup_failed => Color::from_rgb8(136, 90, 34),
            Status::Offline if dark => Color::from_rgb8(214, 145, 127),
            Status::Offline => Color::from_rgb8(158, 67, 48),
        }
    };
    let status_badge = container(label(badge, 11.0).style(move |theme| {
        let tint = status_color(theme);
        let background = mix(colors(theme).surface, tint, 0.12);
        text::Style {
            color: Some(if tint.relative_contrast(background) >= 4.5 {
                tint
            } else {
                iced::theme::palette::readable(background, tint)
            }),
        }
    }))
    .padding([3, 8])
    .style(move |theme| container::Style {
        background: Some(
            Color {
                a: 0.12,
                ..status_color(theme)
            }
            .into(),
        ),
        border: Border {
            radius: 6.0.into(),
            ..Default::default()
        },
        ..Default::default()
    });
    let detail = if app.startup_error.is_some() && app.snapshot.status == Status::Stopped {
        "后台守护已停止，请查看提示并重新启动。"
    } else if app.startup_error.is_some() {
        "打开设置，完善主网络与账号信息。"
    } else if app.snapshot.detail.is_empty() {
        "正在准备连接守护。"
    } else {
        app.snapshot.detail.as_str()
    };
    let last_login = app
        .snapshot
        .last_login
        .and_then(|value| value.elapsed().ok())
        .map(|age| match age.as_secs() {
            0..60 => "刚刚".to_string(),
            60..3600 => format!("{} 分钟前", age.as_secs() / 60),
            _ => format!("{} 小时前", age.as_secs() / 3600),
        })
        .unwrap_or_else(|| "尚无记录".into());
    let next_probe = if app.snapshot.paused {
        "已暂停".to_string()
    } else if app.snapshot.busy {
        "检查中".into()
    } else {
        app.snapshot
            .next_probe
            .map(|next| {
                format!(
                    "{} 秒后",
                    next.saturating_duration_since(Instant::now()).as_secs()
                )
            })
            .unwrap_or_else(|| "等待检查".into())
    };
    let network = if app.snapshot.ssid.is_empty() {
        "未连接 WiFi"
    } else {
        &app.snapshot.ssid
    };
    let recent = column![muted("最近登录", 10.5), label(last_login, 13.0)]
        .spacing(3)
        .width(Fill);
    let upcoming = column![muted("下次检查", 10.5), label(next_probe, 13.0)]
        .spacing(3)
        .width(Fill);
    let wifi = column![
        muted("当前 WiFi", 10.5),
        label(network, 13.0).wrapping(text::Wrapping::Glyph)
    ]
    .spacing(3)
    .width(Fill);
    let metadata: Element<'_, Message> = if status_width < 350.0 {
        column![row![recent, upcoming].spacing(12), wifi]
            .spacing(8)
            .into()
    } else {
        row![recent, upcoming, wifi]
            .spacing(12)
            .align_y(Alignment::Start)
            .into()
    };
    let status_card = container(
        column![
            row![muted("当前连接", 12.0), space().width(Fill), status_badge]
                .align_y(Alignment::Center),
            title(state, 27.0).wrapping(text::Wrapping::Glyph),
            muted(detail, 13.0),
            line(),
            metadata
        ]
        .spacing(7),
    )
    .padding(14)
    .width(Fill)
    .style(surface);

    let login = button(centered(
        if app.snapshot.busy {
            "正在连接…"
        } else {
            "立即登录"
        },
        14.0,
    ))
    .on_press_maybe((!app.snapshot.busy && app.startup_error.is_none()).then_some(Message::Login))
    .width(Fill)
    .padding([8, 14])
    .height(38)
    .style(primary);
    let pause = button(centered(
        if app.snapshot.paused {
            "恢复守护"
        } else {
            "暂停守护"
        },
        14.0,
    ))
    .on_press_maybe(app.startup_error.is_none().then_some(Message::TogglePause))
    .width(Fill)
    .padding([8, 14])
    .height(38)
    .style(secondary);
    let actions: Element<'_, Message> = if compact {
        column![login, pause].spacing(8).into()
    } else {
        row![login, pause].spacing(8).into()
    };

    let mut log_entries = Column::new().spacing(7);
    if app.logs.is_empty() {
        log_entries = log_entries.push(
            column![
                muted("暂无新的动态", 13.0),
                muted("连接状态和操作结果会显示在这里。", 11.0)
            ]
            .spacing(5),
        );
    } else {
        for message in app.logs.iter().rev().take(8) {
            log_entries = log_entries.push(
                row![muted("·", 13.0), muted(message, 13.0)]
                    .spacing(7)
                    .align_y(Alignment::Start),
            );
        }
    }
    let activity = column![muted("最近动态", 12.0), log_entries]
        .spacing(8)
        .width(Fill);
    let fallback = if app.effective.wifi.fallback.ssid.is_empty() {
        "备用网络未设置".to_string()
    } else {
        format!("备用：{}", app.effective.wifi.fallback.ssid)
    };
    let footer = row![
        muted(fallback, 11.0).width(Fill),
        button(label("设置", 13.0))
            .on_press(Message::OpenSettings)
            .padding([7, 11])
            .style(quiet)
    ]
    .align_y(Alignment::Center);

    let mut content = column![
        global_nav(app, compact),
        inset_content(
            header,
            iced::Padding {
                top: if compact { 12.0 } else { 16.0 },
                right: 27.0,
                left: 16.0,
                ..Default::default()
            }
        )
    ]
    .spacing(10);
    if app.preview {
        content = content.push(inset_content(
            muted(
                if compact {
                    "离线预览"
                } else {
                    "离线预览 · 不会认证或切换网络"
                },
                11.0,
            ),
            iced::Padding {
                left: 16.0,
                right: 27.0,
                ..Default::default()
            },
        ));
    }
    let mut body = Column::new().spacing(10);
    if let Some(error) = &app.startup_error {
        body = body.push(notice(error, true));
    } else if let Some(feedback) = &app.feedback
        && feedback_is_error(feedback)
    {
        body = body.push(notice(feedback, true));
    }
    let connection = column![status_card, actions].spacing(10).width(Fill);
    let panels: Element<'_, Message> = if wide {
        row![connection, activity]
            .spacing(20)
            .align_y(Alignment::Start)
            .into()
    } else {
        column![connection, activity].spacing(12).into()
    };
    body = body.push(panels);
    let body = inset_content(
        body,
        iced::Padding {
            left: 16.0,
            right: 16.0,
            bottom: 4.0,
            ..Default::default()
        },
    );
    content
        .push(smooth_scroll::wrap(
            scrollable(body)
                .height(Fill)
                .width(Fill)
                .direction(scrollable::Direction::Vertical(edge_scrollbar()))
                .style(scrollbar),
        ))
        .push(inset_content(
            column![line(), footer].spacing(6),
            iced::Padding {
                left: 16.0,
                right: 27.0,
                bottom: 12.0,
                ..Default::default()
            },
        ))
        .height(Fill)
        .into()
}

fn global_nav(app: &App, compact: bool) -> Element<'static, Message> {
    let size = if compact { 11.0 } else { 12.0 };
    let home = button(centered("网络守护", size))
        .on_press(Message::OpenHome)
        .width(Fill)
        .padding([7, 4])
        .style(if app.page == Page::Home {
            active_tab
        } else {
            inactive_tab
        });
    let running = button(centered("校园跑", size))
        .on_press(Message::OpenRunning)
        .width(Fill)
        .padding([7, 4])
        .style(if app.page == Page::Running {
            active_tab
        } else {
            inactive_tab
        });
    let settings = button(centered("设置", size))
        .on_press(Message::OpenSettings)
        .width(Fill)
        .padding([7, 7])
        .style(inactive_tab);
    row![home, running, settings]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
}

fn active_tab(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = primary(theme, status);
    style.border.radius = iced::border::Radius {
        top_left: 9.0,
        top_right: 9.0,
        bottom_left: 0.0,
        bottom_right: 0.0,
    };
    style
}
fn inactive_tab(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = secondary(theme, status);
    style.border.radius = iced::border::Radius {
        top_left: 9.0,
        top_right: 9.0,
        bottom_left: 0.0,
        bottom_right: 0.0,
    };
    style
}

fn toggle_row<'a>(
    title_text: &'static str,
    description: &'static str,
    value: bool,
    on_toggle: fn(bool) -> Message,
) -> Element<'a, Message> {
    row![
        column![label(title_text, 13.0), muted(description, 11.0)]
            .spacing(4)
            .width(Fill),
        motion::animated_switch(value, on_toggle)
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

fn select_row<'a>(
    compact: bool,
    name: &'static str,
    field: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let field =
        motion::popup_anchor(container(field).width(if compact { Fill } else { 174.0.into() }));
    if compact {
        column![label(name, 13.0), field]
            .spacing(5)
            .width(Fill)
            .into()
    } else {
        row![label(name, 13.0).width(Fill), field]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
    }
}

fn hex_color(value: &str) -> Option<Color> {
    let value = value.strip_prefix('#')?;
    if value.len() != 6 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let rgb = u32::from_str_radix(value, 16).ok()?;
    Some(Color::from_rgb8(
        (rgb >> 16) as u8,
        (rgb >> 8) as u8,
        rgb as u8,
    ))
}

fn color_field<'a>(
    compact: bool,
    name: &'static str,
    value: &'a str,
    on_input: fn(String) -> Message,
    fallback: fn(Colors) -> Color,
) -> Element<'a, Message> {
    let preview = hex_color(value);
    let invalid = preview.is_none();
    let swatch = container(space())
        .width(30)
        .height(34)
        .style(move |theme: &Theme| {
            let p = colors(theme);
            container::Style {
                background: Some(preview.unwrap_or_else(|| fallback(p)).into()),
                border: Border {
                    color: p.border,
                    width: 1.0,
                    radius: 7.0.into(),
                },
                ..Default::default()
            }
        });
    let input = text_input("#RRGGBB", value)
        .font(Font::MONOSPACE)
        .size(13)
        .padding([7, 10])
        .width(Fill)
        .on_input(on_input)
        .style(move |theme, status| {
            let mut style = field(theme, status);
            if invalid {
                style.border.color = theme.palette().danger;
            }
            style
        });
    select_row(
        compact,
        name,
        row![swatch, container(input).width(Fill).max_width(115)]
            .spacing(8)
            .align_y(Alignment::Center),
    )
}

fn appearance(app: &App, compact: bool) -> Element<'_, Message> {
    let section = column![
        muted("外观", 11.5),
        select_row(
            compact,
            "明暗模式",
            pick_list(ThemeMode::ALL, Some(app.form.mode), Message::Mode)
                .width(Fill)
                .padding([7, 10])
                .text_size(13)
                .style(selection)
                .menu_style(menu),
        ),
        select_row(
            compact,
            "设计风格",
            pick_list(ThemeStyle::ALL, Some(app.form.style), Message::Style)
                .width(Fill)
                .padding([7, 10])
                .text_size(13)
                .style(selection)
                .menu_style(menu),
        ),
        select_row(
            compact,
            "显示大小",
            pick_list(
                DisplayScaleChoice::ALL,
                Some(app.form.display_scale),
                Message::DisplayScale,
            )
            .width(Fill)
            .padding([7, 10])
            .text_size(13)
            .style(selection)
            .menu_style(menu),
        ),
        button(label(
            if app.advanced_colors {
                "收起自定义颜色 −"
            } else {
                "自定义颜色 ＋"
            },
            12.0,
        ))
        .padding([6, 0])
        .on_press(Message::ToggleColors)
        .style(quiet)
    ]
    .spacing(10);
    let details = column![
        color_field(
            compact,
            "强调色",
            &app.form.accent,
            Message::Accent,
            |p| p.accent,
        ),
        color_field(
            compact,
            "背景色",
            &app.form.background,
            Message::Background,
            |p| p.background,
        ),
        color_field(
            compact,
            "表面色",
            &app.form.surface,
            Message::Surface,
            |p| p.surface,
        ),
        color_field(
            compact,
            "文字色",
            &app.form.text,
            Message::TextColor,
            |p| p.text,
        ),
        muted("输入 #RRGGBB 即可预览，更改会自动保存。", 11.0),
        button(label("恢复此主题默认颜色", 12.0))
            .padding([6, 0])
            .on_press(Message::ResetColors)
            .style(quiet)
    ]
    .spacing(8);
    column![
        section,
        motion::reveal(
            app.advanced_colors,
            container(details).padding(iced::Padding {
                top: 8.0,
                ..Default::default()
            })
        )
    ]
    .into()
}

fn settings(app: &App) -> Element<'_, Message> {
    responsive(move |size| settings_at_width(app, size.width)).into()
}

fn settings_tabs(app: &App, compact: bool) -> Element<'_, Message> {
    let tabs = if compact {
        [
            (SettingsTab::Global, "全局"),
            (SettingsTab::Network, "校园网"),
            (SettingsTab::Running, "校园跑"),
        ]
    } else {
        [
            (SettingsTab::Global, "全局设置"),
            (SettingsTab::Network, "校园网设置"),
            (SettingsTab::Running, "校园跑设置"),
        ]
    };
    let mut row = iced::widget::Row::new()
        .spacing(if compact { 2 } else { 4 })
        .width(Fill);
    for (tab, name) in tabs {
        row = row.push(
            button(centered(name, 12.0))
                .on_press(Message::SettingsTab(tab))
                .width(Fill)
                .padding([7, 2])
                .style(if app.settings_tab == tab {
                    active_tab
                } else {
                    inactive_tab
                }),
        );
    }
    inset_content(
        row,
        iced::Padding {
            left: 16.0,
            right: 16.0,
            ..Default::default()
        },
    )
}

fn running_settings(app: &App, compact: bool) -> Element<'_, Message> {
    let running = &app.running;
    let password = text_input("校园跑密码", &running.prefs.password)
        .size(13)
        .padding([8, 10])
        .secure(!running.password_visible)
        .on_input(|value| Message::Running(super::running_panel::Message::Password(value)))
        .style(field);
    let default_campuses = running.default_campus_choices();
    let campus_pick = pick_list(
        default_campuses,
        Some(running.selected_default_campus()),
        |choice| Message::Running(super::running_panel::Message::DefaultCampus(choice)),
    )
    .width(Fill)
    .padding([7, 10])
    .text_size(13)
    .style(selection)
    .menu_style(menu);
    let mut fields =
        column![
            muted("校园跑账号", 11.5),
            network_field("手机号", "手机号", &running.prefs.phone, |value| {
                Message::Running(super::running_panel::Message::Phone(value))
            },),
            column![
                label("密码", 12.0),
                row![
                    password,
                    button(label(
                        if running.password_visible {
                            "隐藏"
                        } else {
                            "显示"
                        },
                        12.0
                    ))
                    .on_press(Message::Running(
                        super::running_panel::Message::TogglePassword
                    ))
                    .padding([8, 10])
                    .style(quiet)
                ]
                .spacing(8)
                .align_y(Alignment::Center)
            ]
            .spacing(5),
            select_row(compact, "默认校区", campus_pick),
            column![
                label("路线目录文件", 12.0),
                row![
                    text_input("本地路线 JSON 文件路径", &running.prefs.catalog_file)
                        .size(13)
                        .padding([8, 10])
                        .on_input(|value| Message::Running(
                            super::running_panel::Message::CatalogFile(value)
                        ))
                        .style(field),
                    button(label("载入", 12.0))
                        .on_press_maybe((!running.busy()).then_some(Message::Running(
                            super::running_panel::Message::LoadCatalog
                        )))
                        .padding([8, 10])
                        .style(secondary)
                ]
                .spacing(8)
                .align_y(Alignment::Center)
            ]
            .spacing(5),
            muted(
                "留空使用航空港与龙泉校区底图；本次校区不会修改默认校区。",
                11.0
            )
        ]
        .spacing(10);
    if let Some(error) = &running.error {
        fields = fields.push(notice(error, true));
    } else if let Some(status) = &running.status {
        fields = fields.push(muted(status, 11.0));
    }
    fields.into()
}

fn running(app: &App) -> Element<'_, Message> {
    responsive(move |size| running_at_size(app, size)).into()
}

fn running_at_size(app: &App, size: iced::Size) -> Element<'_, Message> {
    use super::running_panel::Message as RunMessage;
    let width = size.width;
    let compact = width < COMPACT_WIDTH;
    let wide = width >= RUNNING_WIDE_WIDTH;
    let state = &app.running;
    let map_width = if wide {
        (width - 32.0).min(CONTENT_MAX_WIDTH) - 316.0
    } else {
        (width - 32.0).min(CONTENT_MAX_WIDTH)
    };
    let map_aspect = state
        .preview_for_plot()
        .and_then(|preview| preview.map.as_ref())
        .map(|map| ((map.bounds[2] - map.bounds[0]) / (map.bounds[3] - map.bounds[1])) as f32)
        .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
        .unwrap_or(1.5);
    let available_height = if wide {
        (size.height - 220.0).clamp(220.0, 420.0)
    } else {
        (size.height * 0.65).clamp(220.0, 420.0)
    };
    let plot_height = ((map_width - 24.0).max(1.0) / map_aspect + 84.0)
        .min(available_height)
        .clamp(150.0, 420.0);
    let campuses = state.campus_choices();
    let routes = state.route_choices();
    let campus = pick_list(campuses, Some(state.selected_campus()), |choice| {
        Message::Running(RunMessage::Campus(choice))
    })
    .width(Fill)
    .padding([7, 10])
    .text_size(13)
    .style(selection)
    .menu_style(menu);
    let route = pick_list(routes, Some(state.selected_route()), |choice| {
        Message::Running(RunMessage::Route(choice))
    })
    .width(Fill)
    .padding([7, 10])
    .text_size(13)
    .style(selection)
    .menu_style(menu);
    let noise = pick_list(crate::running::Noise::ALL, Some(state.noise), |noise| {
        Message::Running(RunMessage::Noise(noise))
    })
    .width(Fill)
    .padding([7, 10])
    .text_size(13)
    .style(selection)
    .menu_style(menu);
    let distance = network_field(
        "目标距离（米）",
        "例如 2000",
        &state.distance,
        |value| Message::Running(RunMessage::Distance(value)),
    );
    let minutes = network_field(
        "目标时间（分钟）",
        "例如 25",
        &state.minutes,
        |value| Message::Running(RunMessage::Minutes(value)),
    );
    let inputs: Element<'_, Message> = if width < 380.0 || wide {
        column![distance, minutes].spacing(8).into()
    } else {
        row![distance, minutes].spacing(10).into()
    };
    let controls = column![
        select_row(compact, "本次校区", campus),
        select_row(compact, "路线", route),
        inputs,
        select_row(compact, "路线扰动", noise),
        toggle_row(
            "随机距离与时长",
            "模拟配速 6–9 分钟/公里",
            state.random_parameters,
            |value| Message::Running(RunMessage::RandomParameters(value))
        )
    ]
    .spacing(9);
    let plot = state.plot.view(
        state.preview_for_plot(),
        state.progress,
        state.revision,
        plot_height,
    );
    let mut metrics = column![plot].spacing(9);
    if state.noise == crate::running::Noise::Strong {
        metrics = metrics.push(muted(
            "强扰动最大20米，可能偏出跑道，仅用于本地模拟。",
            11.0,
        ));
    }
    if let Some(preview) = &state.preview {
        let km = preview.distance_m / 1000.0;
        let seconds = preview.duration_seconds;
        let pace = if km > 0.0 { seconds as f64 / km } else { 0.0 };
        let stats = row![
            column![
                muted("预览里程", 10.5),
                label(format!("{km:.2} 公里"), 13.0)
            ]
            .spacing(3)
            .width(Fill),
            column![
                muted("预计时间", 10.5),
                label(format!("{} 分钟", seconds / 60), 13.0)
            ]
            .spacing(3)
            .width(Fill),
            column![
                muted("配速", 10.5),
                label(
                    format!("{}′{:02}″/公里", (pace / 60.0) as u32, (pace as u32) % 60),
                    13.0
                )
            ]
            .spacing(3)
            .width(Fill)
        ]
        .spacing(8);
        let stats: Element<'_, Message> = if compact {
            column![
                row![
                    column![
                        muted("预览里程", 10.5),
                        label(format!("{km:.2} 公里"), 13.0)
                    ]
                    .spacing(3)
                    .width(Fill),
                    column![
                        muted("预计时间", 10.5),
                        label(format!("{} 分钟", seconds / 60), 13.0)
                    ]
                    .spacing(3)
                    .width(Fill)
                ]
                .spacing(8),
                column![
                    muted("配速", 10.5),
                    label(
                        format!("{}′{:02}″/公里", (pace / 60.0) as u32, (pace as u32) % 60),
                        13.0
                    )
                ]
                .spacing(3)
            ]
            .spacing(8)
            .into()
        } else {
            stats.into()
        };
        metrics = metrics.push(stats);
    }
    metrics = metrics.push(
        iced::widget::slider(0.0..=100.0, state.progress, |value| {
            Message::Running(RunMessage::Progress(value))
        })
        .step(1.0_f32),
    );
    if let Some(error) = &state.error {
        metrics = metrics.push(notice(error, true));
    }
    if let Some(status) = &state.status {
        metrics = metrics.push(notice(status, false));
    }
    let generate = button(centered("生成路线预览", 13.0))
        .on_press_maybe((!state.busy()).then_some(Message::Running(RunMessage::Generate)))
        .width(Fill)
        .padding([8, 12])
        .style(primary);
    let cancel = button(centered("取消", 13.0))
        .on_press_maybe(state.busy().then_some(Message::Running(RunMessage::Cancel)))
        .width(Fill)
        .padding([8, 12])
        .style(secondary);
    let plot_area = column![
        metrics,
        muted("生成数据为本地模拟，不会提交运动记录。", 11.0)
    ]
    .spacing(12)
    .width(Fill);
    let main_content: Element<'_, Message> = if wide {
        row![
            container(controls).width(300),
            container(plot_area).width(Fill)
        ]
        .spacing(16)
        .into()
    } else {
        column![controls, line(), plot_area].spacing(12).into()
    };
    let body = inset_content(
        main_content,
        iced::Padding {
            left: 16.0,
            right: 16.0,
            top: 12.0,
            bottom: 8.0,
        },
    );
    column![
        global_nav(app, compact),
        inset_content(
            title("校园跑", 21.0),
            iced::Padding {
                left: 16.0,
                right: 16.0,
                top: 10.0,
                ..Default::default()
            }
        ),
        line(),
        container(scroll_edge::wrap(smooth_scroll::wrap(
            scrollable(body)
                .height(Fill)
                .width(Fill)
                .direction(scrollable::Direction::Vertical(edge_scrollbar()))
                .style(scrollbar)
        )))
        .height(Fill),
        line(),
        inset_content(
            row![generate, cancel].spacing(8),
            iced::Padding {
                left: 16.0,
                right: 16.0,
                bottom: 10.0,
                top: 8.0,
            }
        )
    ]
    .height(Fill)
    .into()
}

fn network_field<'a>(
    name: &'static str,
    placeholder: &'static str,
    value: &'a str,
    on_input: fn(String) -> Message,
) -> Element<'a, Message> {
    column![
        label(name, 12.0),
        text_input(placeholder, value)
            .size(13)
            .padding([8, 10])
            .on_input(on_input)
            .style(field)
    ]
    .spacing(5)
    .into()
}

fn primary_network(app: &App, compact: bool) -> Element<'_, Message> {
    let password = text_input("校园网密码", &app.form.account_password)
        .size(13)
        .padding([8, 10])
        .secure(!app.form.account_password_visible)
        .on_input(Message::AccountPassword)
        .style(field);
    let main = column![
        muted("主网络", 11.5),
        muted("请先在 Windows 中连接并保存该 WiFi。", 11.0),
        network_field(
            "主 WiFi 名称",
            "Windows 已保存的 WiFi 名称",
            &app.form.primary_ssid,
            Message::PrimarySsid
        ),
        network_field(
            "校园账号",
            "校园网账号",
            &app.form.username,
            Message::Username
        ),
        column![
            label("校园密码", 12.0),
            row![
                password,
                button(label(
                    if app.form.account_password_visible {
                        "隐藏"
                    } else {
                        "显示"
                    },
                    13.0
                ))
                .on_press(Message::ToggleAccountPassword)
                .padding([8, 10])
                .style(quiet)
            ]
            .spacing(8)
            .align_y(Alignment::Center)
        ]
        .spacing(5),
        select_row(
            compact,
            "运营商",
            pick_list(ServiceChoice::ALL, Some(app.form.service), Message::Service)
                .width(Fill)
                .padding([7, 10])
                .text_size(13)
                .style(selection)
                .menu_style(menu)
        ),
        muted("主网络修改自动保存，重启后生效。", 11.0),
        button(label(
            if app.advanced_portal {
                "收起门户参数 −"
            } else {
                "门户参数 ＋"
            },
            12.0
        ))
        .on_press(Message::TogglePortal)
        .padding([6, 0])
        .style(quiet)
    ]
    .spacing(8);
    let portal = column![
        network_field(
            "认证入口",
            "https://…",
            &app.form.entry_url,
            Message::EntryUrl
        ),
        network_field(
            "认证接口",
            "https://…",
            &app.form.api_base,
            Message::ApiBase
        ),
        network_field(
            "连通性探测",
            "http://…",
            &app.form.probe_url,
            Message::ProbeUrl
        ),
    ]
    .spacing(8);
    column![
        main,
        motion::reveal(
            app.advanced_portal,
            container(portal).padding(iced::Padding {
                top: 8.0,
                ..Default::default()
            })
        )
    ]
    .into()
}

fn settings_at_width(app: &App, width: f32) -> Element<'_, Message> {
    let compact = width < COMPACT_WIDTH;
    let mut heading = column![title(
        if compact {
            "设置"
        } else {
            "CUIT Helper 设置"
        },
        22.0,
    )]
    .spacing(5)
    .width(Fill);
    if !compact {
        heading = heading.push(muted("按照你的习惯设置 CUIT Helper。", 12.0));
    }
    let header = row![
        heading,
        button(label("返回", 13.0))
            .on_press(Message::Back)
            .padding([8, 16])
            .style(secondary)
    ]
    .align_y(Alignment::Center);

    let start = column![
        muted("启动", 11.5),
        toggle_row(
            "登录时自启",
            "登录 Windows 后在托盘运行",
            app.form.autostart,
            Message::Autostart,
        )
    ]
    .spacing(10);

    let fallback_header = column![
        muted("备用网络", 11.5),
        toggle_row(
            "使用备用 WiFi",
            "校园网不可用时自动切换",
            app.form.fallback_enabled,
            Message::Fallback,
        )
    ]
    .spacing(10);
    let fallback_details = {
        let ssid = text_input("填写备用 WiFi 名称", &app.form.ssid)
            .size(13)
            .padding([8, 10])
            .on_input(Message::Ssid)
            .style(field);
        let password = text_input("填写热点密码", &app.form.password)
            .secure(!app.form.password_visible)
            .size(13)
            .padding([8, 10])
            .on_input(Message::Password)
            .style(field);
        let mut details = Column::new()
            .spacing(8)
            .push(
                column![
                    label("WiFi 名称", 12.0),
                    row![
                        ssid,
                        button(label("扫描", 13.0))
                            .on_press(Message::Scan)
                            .padding([8, 10])
                            .style(secondary)
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center)
                ]
                .spacing(7),
            )
            .push(
                column![
                    label("热点密码", 12.0),
                    row![
                        password,
                        button(label(
                            if app.form.password_visible {
                                "隐藏"
                            } else {
                                "显示"
                            },
                            12.0
                        ))
                        .on_press(Message::TogglePassword)
                        .padding([8, 10])
                        .style(quiet)
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center)
                ]
                .spacing(7),
            );
        if !app.networks.is_empty() {
            details = details.push(motion::popup_anchor_revision(
                pick_list(app.networks.as_slice(), None::<String>, Message::Ssid)
                    .placeholder("选择扫描到的 WiFi")
                    .width(Fill)
                    .padding([8, 10])
                    .text_size(13)
                    .style(selection)
                    .menu_style(menu),
                app.networks_revision,
            ));
        }
        details.push(select_row(
            compact,
            "切回校园网",
            pick_list(
                RecoveryChoice::ALL,
                Some(app.form.recovery),
                Message::Recovery,
            )
            .width(Fill)
            .padding([7, 10])
            .text_size(13)
            .style(selection)
            .menu_style(menu),
        ))
    };
    let fallback = column![
        fallback_header,
        motion::reveal(
            app.form.fallback_enabled,
            container(fallback_details).padding(iced::Padding {
                top: 8.0,
                ..Default::default()
            })
        )
    ];

    let guardian = column![
        muted("连接守护", 11.5),
        select_row(
            compact,
            "探测间隔",
            pick_list(
                IntervalChoice::ALL,
                Some(app.form.interval),
                Message::Interval,
            )
            .width(Fill)
            .padding([7, 10])
            .text_size(13)
            .style(selection)
            .menu_style(menu),
        ),
        select_row(
            compact,
            "掉线判定",
            pick_list(
                ThresholdChoice::ALL,
                Some(app.form.threshold),
                Message::Threshold,
            )
            .width(Fill)
            .padding([7, 10])
            .text_size(13)
            .style(selection)
            .menu_style(menu),
        ),
        toggle_row(
            "调试日志",
            "需要排查连接问题时开启",
            app.form.debug,
            Message::Debug,
        )
    ]
    .spacing(10);

    let open_config = button(centered("打开配置文件", 13.0))
        .on_press(Message::OpenConfig)
        .width(Fill)
        .padding([8, 10])
        .style(secondary);
    let open_log = button(centered("打开日志", 13.0))
        .on_press(Message::OpenLog)
        .width(Fill)
        .padding([8, 10])
        .style(secondary);
    let file_actions: Element<'_, Message> = if compact {
        column![open_config, open_log].spacing(8).into()
    } else {
        row![open_config, open_log].spacing(10).into()
    };
    let files = column![
        muted("配置与日志", 11.5),
        file_actions,
        muted("超时、重试等高级参数可在配置文件中调整。", 11.0)
    ]
    .spacing(10);

    let global_body: Element<'_, Message> =
        column![appearance(app, compact), line(), start, line(), files]
            .spacing(12)
            .into();
    let network_body: Element<'_, Message> = column![
        primary_network(app, compact),
        line(),
        fallback,
        line(),
        guardian
    ]
    .spacing(12)
    .into();
    let running_body: Element<'_, Message> = running_settings(app, compact);
    let body: Element<'_, Message> = match app.settings_tab {
        SettingsTab::Global => global_body,
        SettingsTab::Network => network_body,
        SettingsTab::Running => running_body,
    };
    let body = inset_content(
        body,
        iced::Padding {
            left: 16.0,
            right: 16.0,
            bottom: 8.0,
            top: 12.0,
        },
    );
    let mut page = column![
        inset_content(
            header,
            iced::Padding {
                top: if compact { 12.0 } else { 16.0 },
                right: 27.0,
                left: 16.0,
                ..Default::default()
            }
        ),
        settings_tabs(app, compact)
    ]
    .spacing(if compact { 8 } else { 12 });
    if app.preview {
        page = page.push(inset_content(
            muted(
                if compact {
                    "离线预览"
                } else {
                    "离线预览 · 设置不会写入文件或改变网络"
                },
                11.0,
            ),
            iced::Padding {
                left: 16.0,
                right: 27.0,
                ..Default::default()
            },
        ));
    }
    page = page.push(line());
    let status = if app.pending {
        "正在保存…"
    } else {
        app.feedback.as_deref().unwrap_or("更改会自动保存")
    };
    let failed = !app.pending && feedback_is_error(status);
    let footer = inset_content(
        label(status, 11.0).style(move |theme: &Theme| text::Style {
            color: Some(if failed {
                theme.palette().danger
            } else {
                colors(theme).muted
            }),
        }),
        iced::Padding {
            left: 16.0,
            right: 27.0,
            bottom: 12.0,
            ..Default::default()
        },
    );
    let scrolling = scroll_edge::wrap(smooth_scroll::wrap(
        scrollable(body)
            .height(Fill)
            .width(Fill)
            .direction(scrollable::Direction::Vertical(edge_scrollbar()))
            .style(scrollbar),
    ));
    column![
        page,
        container(scrolling).height(Fill).padding(iced::Padding {
            top: 4.0,
            ..Default::default()
        }),
        line(),
        container(footer).padding(iced::Padding {
            top: 6.0,
            ..Default::default()
        })
    ]
    .height(Fill)
    .into()
}

fn exit_confirmation(app: &App) -> Element<'_, Message> {
    responsive(move |size| exit_confirmation_at_size(app, size)).into()
}

fn exit_confirmation_at_size(app: &App, size: iced::Size) -> Element<'_, Message> {
    let compact = size.width < COMPACT_WIDTH;
    let exiting = app.shutdown.is_some();
    let mut content = column![
        title(
            if exiting {
                "正在保存并退出…"
            } else {
                "退出 CUIT Helper？"
            },
            22.0
        ),
        muted(
            if exiting {
                "正在保存剩余更改，请稍候。"
            } else {
                "退出后将停止自动重连。\n关闭面板可以让守护继续在托盘运行。"
            },
            13.0
        )
    ]
    .spacing(14);
    if let Some(error) = &app.exit_error {
        content = content.push(notice(error, true));
    }
    let cancel = button(centered(
        if app.exit_error.is_some() {
            "返回"
        } else {
            "继续守护"
        },
        13.0,
    ))
    .on_press_maybe((!exiting).then_some(Message::CancelExit))
    .width(Fill)
    .padding([8, 10])
    .style(secondary);
    let confirm = button(centered(
        if app.exit_error.is_some() {
            "重试保存并退出"
        } else {
            "确认退出"
        },
        13.0,
    ))
    .on_press_maybe((!exiting).then_some(Message::ConfirmExit))
    .width(Fill)
    .padding([8, 10])
    .style(primary);
    let actions: Element<'_, Message> = if compact {
        column![cancel, confirm].spacing(8).into()
    } else {
        row![cancel, confirm].spacing(10).into()
    };
    content = content.push(actions);
    if app.discard_on_exit {
        content = content.push(
            button(centered("放弃修改并退出", 12.0))
                .on_press_maybe((!exiting).then_some(Message::DiscardAndExit))
                .width(Fill)
                .padding([6, 10])
                .style(quiet),
        );
    }
    let dialog = container(smooth_scroll::wrap(
        scrollable(content)
            .height(iced::Shrink)
            .width(Fill)
            .direction(scrollable::Direction::Vertical(edge_scrollbar()))
            .style(scrollbar),
    ))
    .padding(if compact { 12 } else { 20 })
    .max_width(416)
    .max_height((size.height - 32.0).max(0.0))
    .style(surface);
    container(dialog).padding(16).center(Fill).into()
}

fn feedback_is_error(message: &str) -> bool {
    ["失败", "错误", "无效", "无法", "尚未保存"]
        .iter()
        .any(|word| message.contains(word))
}
