//! Lucide icons (ISC, see assets/icons/LICENSE-lucide.txt), drawn with the text colour of their button.

use freya::prelude::{Color, ContainerSizeExt, Size, SvgViewer};

#[derive(Clone, Copy)]
pub struct Icon {
    name: &'static str,
    data: &'static [u8],
}

macro_rules! icons {
    ($($constant:ident = $file:literal),* $(,)?) => {
        $(pub const $constant: Icon = Icon {
            name: $file,
            data: include_bytes!(concat!("../../assets/icons/", $file, ".svg")),
        };)*
    };
}

icons! {
    ADD_GAME = "folder-plus",
    FIND_GAMES = "folder-search",
    FAST_SCAN = "zap",
    DEEP_SCAN = "scan-search",
    CLEAR = "eraser",
    APPLY = "check",
    RESTORE = "rotate-ccw",
    MARK = "hash",
    UNMARK = "undo-2",
    EXPORT = "package",
    SAVE = "download",
    STOP = "square",
    ADD_FONT = "file-plus",
    TRASH = "trash",
}

const SIZE: f32 = 16.;

#[must_use]
pub fn draw(icon: Icon, color: Color) -> SvgViewer {
    SvgViewer::new((icon.name, icon.data))
        .width(Size::px(SIZE))
        .height(Size::px(SIZE))
        .stroke(color)
}
