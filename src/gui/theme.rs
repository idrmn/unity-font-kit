//! Surfaces step up in lightness from sunken to raised; shadows mark only what floats above the window.

use super::icons::{self, Icon};
use freya::prelude::{
    Alignment, Border, BorderAlignment, BorderWidth, Button, ButtonColorsThemePartial,
    ButtonColorsThemePreference, ButtonLayoutThemePreference, CheckboxThemePreference, ChildrenExt,
    Color, ColorsSheet, ContainerSizeExt, ContainerWithContentExt, CornerRadius, FontWeight, Gaps,
    InputColorsThemePreference, InputLayoutThemePreference, Label, MenuContainerThemePreference,
    MenuItemThemePreference, PopupThemePreference, Preference, ProgressBarThemePreference, Rect,
    ScrollBarThemePreference, SelectThemePreference, Shadow, Size, StyleExt,
    SwitchColorsThemePreference, TextOverflow, TextStyleExt, Theme, TooltipThemePreference, label,
    light_theme, rect,
};

pub const FONT: &str = "IBM Plex Sans";
pub const FONTS: [(&str, &[u8]); 3] = [
    (FONT, crate::fonts::numbers::LETTERS),
    (
        FONT,
        include_bytes!("../../assets/fonts/IBMPlexSans-Medium.otf"),
    ),
    (
        FONT,
        include_bytes!("../../assets/fonts/IBMPlexSans-SemiBold.otf"),
    ),
];

// IBM Carbon's cool grays, about 6% lighter per level so the text on the top levels keeps its contrast.
pub const SUNKEN: Color = Color::from_rgb(208, 213, 220);
pub const WINDOW: Color = Color::from_rgb(221, 225, 230);
pub const CONTENT: Color = Color::from_rgb(238, 241, 245);
pub const RAISED: Color = Color::from_rgb(252, 253, 254);
pub const OVERLAY: Color = Color::from_rgb(255, 255, 255);
// A chosen item comes up one level, to the colour of the content beside it.
pub const SELECTION: Color = CONTENT;
pub const SPECIMEN_TEXT: Color = Color::from_rgb(244, 244, 244);

pub const TEXT: Color = Color::from_rgb(18, 22, 25);
pub const SECONDARY: Color = Color::from_rgb(77, 83, 88);
pub const TERTIARY: Color = Color::from_rgb(105, 112, 119);
pub const SEPARATOR: Color = Color::from_argb(26, 18, 22, 25);
pub const CONTROL_BORDER: Color = Color::from_argb(46, 18, 22, 25);
pub const HOVER: Color = Color::from_argb(13, 18, 22, 25);
// Controls are tinted, not white: the same fill reads on every surface, and hover darkens it a step.
pub const CONTROL_FILL: Color = Color::from_argb(15, 18, 22, 25);
pub const CONTROL_HOVER: Color = Color::from_argb(31, 18, 22, 25);

pub const ACCENT: Color = Color::from_rgb(15, 98, 254);
pub const ACCENT_HOVER: Color = Color::from_rgb(3, 83, 233);
pub const ACCENT_TINT: Color = Color::from_argb(26, 15, 98, 254);
const FOCUS_RING: Color = Color::from_argb(115, 15, 98, 254);

pub const OK: Color = Color::from_rgb(25, 128, 56);
pub const WARN: Color = Color::from_rgb(142, 106, 0);
pub const ERROR: Color = Color::from_rgb(218, 30, 40);
pub const ERROR_FILL: Color = Color::from_rgb(218, 30, 40);
pub const ERROR_HOVER: Color = Color::from_rgb(184, 25, 33);
pub const ERROR_TINT: Color = Color::from_argb(26, 218, 30, 40);

pub const BODY: f32 = 15.;
pub const CAPTION: f32 = 12.;
pub const DIALOG_TITLE: f32 = 19.;

pub const CONTROL_RADIUS: f32 = 6.;
pub const PANEL_RADIUS: f32 = 10.;
pub const DIALOG_RADIUS: f32 = 14.;
// 0.6 matches Figma's "iOS" corner smoothing: a squircle rather than a rounded rectangle.
const SMOOTHING: f32 = 0.6;

#[must_use]
pub fn corners() -> CornerRadius {
    rounded(CONTROL_RADIUS)
}

#[must_use]
pub fn rounded(radius: f32) -> CornerRadius {
    CornerRadius::new_all(radius).with_smoothing(SMOOTHING)
}

#[must_use]
pub fn hairline(color: Color) -> Border {
    Border::new()
        .fill(color)
        .width(1.)
        .alignment(BorderAlignment::Inner)
}

#[derive(Clone, Copy)]
pub enum Side {
    Top,
    Right,
}

#[must_use]
pub fn edge(side: Side) -> Border {
    let (top, right) = match side {
        Side::Top => (1., 0.),
        Side::Right => (0., 1.),
    };
    hairline(SEPARATOR).width(BorderWidth {
        top,
        right,
        bottom: 0.,
        left: 0.,
    })
}

fn shadow(y: f32, blur: f32, spread: f32, alpha: u8) -> Shadow {
    Shadow::new()
        .y(y)
        .blur(blur)
        .spread(spread)
        .color(Color::from_argb(alpha, 18, 22, 25))
}

#[must_use]
pub fn raised(card: Rect) -> Rect {
    card.background(RAISED)
        .border(hairline(SEPARATOR))
        .shadow(shadow(1., 2., 0., 15))
}

#[must_use]
pub fn modal(card: Rect) -> Rect {
    card.background(OVERLAY)
        .corner_radius(rounded(DIALOG_RADIUS))
        .border(hairline(SEPARATOR))
        .shadow(shadow(8., 24., 0., 31))
        .shadow(shadow(24., 60., 0., 46))
}

#[must_use]
pub fn text(value: impl Into<String>, size: f32, color: Color) -> Rect {
    rect().font_size(size).color(color).child(value.into())
}

#[must_use]
pub fn strong(value: impl Into<String>, size: f32) -> Rect {
    text(value, size, TEXT).font_weight(FontWeight::SEMI_BOLD)
}

#[must_use]
pub fn caption(value: impl Into<String>) -> Rect {
    text(value, CAPTION, SECONDARY).font_weight(FontWeight::MEDIUM)
}

#[must_use]
pub fn dot(color: Color) -> Rect {
    rect()
        .width(Size::px(7.))
        .height(Size::px(7.))
        .corner_radius(999.)
        .background(color)
}

/// A short vertical line between groups of buttons.
#[must_use]
pub fn separator() -> Rect {
    rect()
        .width(Size::px(1.))
        .height(Size::px(24.))
        .background(SEPARATOR)
}

#[must_use]
pub fn divider() -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::px(1.))
        .background(SEPARATOR)
}

/// Freya draws a disabled button like an enabled one, so it is faded here.
#[must_use]
pub fn dimmed(enabled: bool, button: Button) -> Rect {
    rect()
        .opacity(if enabled { 1. } else { 0.45 })
        .child(button.enabled(enabled))
}

#[must_use]
pub fn primary(label: &str) -> Button {
    Button::new().filled().child(label.to_string())
}

#[must_use]
pub fn secondary(label: &str) -> Button {
    Button::new().child(label.to_string())
}

#[must_use]
pub fn primary_with(icon: Icon, label: &str) -> Button {
    Button::new()
        .filled()
        .child(labelled(icon, label, Color::WHITE))
}

#[must_use]
pub fn secondary_with(icon: Icon, label: &str) -> Button {
    Button::new().child(labelled(icon, label, TEXT))
}

fn labelled(icon: Icon, label: &str, color: Color) -> Rect {
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(8.)
        .child(icons::draw(icon, color))
        .child(label.to_string())
}

#[must_use]
pub fn destructive(label: &str) -> Button {
    Button::new()
        .theme_colors(ButtonColorsThemePartial {
            background: Some(Preference::Specific(ERROR_FILL)),
            hover_background: Some(Preference::Specific(ERROR_HOVER)),
            border_fill: Some(Preference::Specific(Color::TRANSPARENT)),
            color: Some(Preference::Specific(Color::WHITE)),
            ..Default::default()
        })
        .child(label.to_string())
}

#[must_use]
pub fn line(value: impl Into<String>, size: f32, color: Color) -> Label {
    label()
        .text(value.into())
        .max_lines(1)
        .text_overflow(TextOverflow::Ellipsis)
        .width(Size::fill())
        .font_size(size)
        .color(color)
}

#[must_use]
pub fn build() -> Theme {
    let mut theme = light_theme();
    theme.colors = palette(theme.colors);
    set_buttons(&mut theme);
    set_controls(&mut theme);
    theme
}

fn palette(base: ColorsSheet) -> ColorsSheet {
    ColorsSheet {
        primary: ACCENT,
        secondary: RAISED,
        tertiary: ACCENT_HOVER,
        success: OK,
        warning: WARN,
        error: ERROR,
        background: RAISED,
        surface_primary: SELECTION,
        surface_secondary: OVERLAY,
        surface_tertiary: RAISED,
        surface_inverse: TERTIARY,
        surface_inverse_secondary: SECONDARY,
        surface_inverse_tertiary: TEXT,
        border: CONTROL_BORDER,
        border_focus: FOCUS_RING,
        border_disabled: SEPARATOR,
        text_primary: TEXT,
        text_secondary: SECONDARY,
        text_placeholder: TERTIARY,
        text_inverse: Color::WHITE,
        text_highlight: ACCENT,
        focus: ACCENT_TINT,
        active: SELECTION,
        disabled: RAISED,
        shadow: Color::from_argb(26, 18, 22, 25),
        ..base
    }
}

fn set_buttons(theme: &mut Theme) {
    let layout = |padding: Gaps, width: Size| ButtonLayoutThemePreference {
        padding: Preference::Specific(padding),
        margin: Preference::Specific(Gaps::new_all(0.)),
        corner_radius: Preference::Specific(corners()),
        width: Preference::Specific(width),
        height: Preference::Specific(Size::Inner),
    };
    theme.set(
        "button_layout",
        layout(Gaps::new(8., 16., 8., 16.), Size::Inner),
    );
    theme.set(
        "compact_button_layout",
        layout(Gaps::new(4., 12., 4., 12.), Size::Inner),
    );
    theme.set(
        "expanded_button_layout",
        layout(Gaps::new(10., 16., 10., 16.), Size::fill()),
    );
    theme.set(
        "button",
        ButtonColorsThemePreference {
            background: Preference::Specific(CONTROL_FILL),
            hover_background: Preference::Specific(CONTROL_HOVER),
            border_fill: Preference::Specific(Color::TRANSPARENT),
            focus_border_fill: Preference::Specific(FOCUS_RING),
            color: Preference::Specific(TEXT),
        },
    );
    theme.set(
        "flat_button",
        ButtonColorsThemePreference {
            background: Preference::Specific(Color::TRANSPARENT),
            hover_background: Preference::Specific(CONTROL_FILL),
            border_fill: Preference::Specific(Color::TRANSPARENT),
            focus_border_fill: Preference::Specific(FOCUS_RING),
            color: Preference::Specific(SECONDARY),
        },
    );
}

fn set_controls(theme: &mut Theme) {
    for (name, margin) in [
        ("input_layout", Gaps::new(9., 12., 9., 12.)),
        ("compact_input_layout", Gaps::new(4., 8., 4., 8.)),
        ("expanded_input_layout", Gaps::new(10., 12., 10., 12.)),
    ] {
        theme.set(
            name,
            InputLayoutThemePreference {
                corner_radius: Preference::Specific(corners()),
                inner_margin: Preference::Specific(margin),
            },
        );
    }
    theme.set(
        "input",
        InputColorsThemePreference {
            background: Preference::Specific(CONTROL_FILL),
            focus_background: Preference::Specific(RAISED),
            color: Preference::Specific(TEXT),
            placeholder_color: Preference::Specific(TERTIARY),
            border_fill: Preference::Specific(Color::TRANSPARENT),
            focus_border_fill: Preference::Specific(FOCUS_RING),
        },
    );
    theme.set(
        "select",
        SelectThemePreference {
            width: Preference::Specific(Size::Inner),
            margin: Preference::Specific(Gaps::new_all(0.)),
            select_background: Preference::Specific(OVERLAY),
            background_button: Preference::Specific(CONTROL_FILL),
            hover_background: Preference::Specific(CONTROL_HOVER),
            color: Preference::Specific(TEXT),
            border_fill: Preference::Specific(Color::TRANSPARENT),
            focus_border_fill: Preference::Specific(FOCUS_RING),
            arrow_fill: Preference::Specific(SECONDARY),
        },
    );
    theme.set(
        "checkbox",
        CheckboxThemePreference {
            unselected_fill: Preference::Specific(CONTROL_BORDER),
            selected_fill: Preference::Specific(ACCENT),
            selected_icon_fill: Preference::Specific(Color::WHITE),
            border_fill: Preference::Specific(FOCUS_RING),
        },
    );
    theme.set(
        "switch",
        SwitchColorsThemePreference {
            background: Preference::Specific(Color::from_rgb(193, 199, 205)),
            thumb_background: Preference::Specific(RAISED),
            toggled_background: Preference::Specific(ACCENT),
            toggled_thumb_background: Preference::Specific(Color::WHITE),
            focus_border_fill: Preference::Specific(FOCUS_RING),
        },
    );
    theme.set(
        "menu_item",
        MenuItemThemePreference {
            background: Preference::Specific(Color::TRANSPARENT),
            hover_background: Preference::Specific(HOVER),
            select_background: Preference::Specific(ACCENT_TINT),
            border_fill: Preference::Specific(Color::TRANSPARENT),
            select_border_fill: Preference::Specific(FOCUS_RING),
            corner_radius: Preference::Specific(corners()),
            color: Preference::Specific(TEXT),
        },
    );
    theme.set(
        "menu_container",
        MenuContainerThemePreference {
            background: Preference::Specific(OVERLAY),
            padding: Preference::Specific(Gaps::new_all(4.)),
            shadow: Preference::Specific(Color::from_argb(31, 18, 22, 25)),
            border_fill: Preference::Specific(SEPARATOR),
            corner_radius: Preference::Specific(rounded(PANEL_RADIUS)),
        },
    );
    theme.set(
        "tooltip",
        TooltipThemePreference {
            background: Preference::Specific(Color::from_rgb(52, 58, 63)),
            color: Preference::Specific(SPECIMEN_TEXT),
            border_fill: Preference::Specific(Color::TRANSPARENT),
            font_size: Preference::Specific(CAPTION + 1.),
        },
    );
    theme.set(
        "scrollbar",
        ScrollBarThemePreference {
            // Freya swaps this colour's alpha for its own on hover, so a transparent black turns dark grey.
            background: Preference::Specific(CONTENT),
            thumb_background: Preference::Specific(CONTROL_BORDER),
            hover_thumb_background: Preference::Specific(TERTIARY),
            active_thumb_background: Preference::Specific(SECONDARY),
            size: Preference::Specific(10.),
        },
    );
    theme.set(
        "progressbar",
        ProgressBarThemePreference {
            color: Preference::Specific(Color::TRANSPARENT),
            background: Preference::Specific(SUNKEN),
            progress_background: Preference::Specific(ACCENT),
            height: Preference::Specific(8.),
        },
    );
    theme.set(
        "popup",
        PopupThemePreference {
            background: Preference::Specific(Color::TRANSPARENT),
            color: Preference::Specific(TEXT),
            width: Preference::Specific(Size::px(500.)),
            height: Preference::Specific(Size::auto()),
            padding: Preference::Specific(Gaps::new_all(0.)),
            spacing: Preference::Specific(0.),
        },
    );
}
