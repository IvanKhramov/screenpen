//! English and Russian UI strings.

use screenpen_core::{Action, ToolKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
}

/// Every fixed UI string. Action and tool names go through `action_label`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum T {
    Draw,
    GhostMenu,
    Whiteboard,
    DimMenu,
    DimOff,
    DimSlightly,
    DimMedium,
    DimStrongly,
    FadeMenu,
    FadeOff,
    /// "After {n} s"
    FadeAfter,
    HidePalette,
    Autostart,
    SnapshotSaved,
    SnapshotFailed,
    Settings,
    Quit,
    Running,
    AlreadyRunning,
    /// "{key} or click the tray icon to draw."
    HowToDraw,
    HotkeyTaken,
    /// "Taken by another app: {keys}. Use the tray icon."
    HotkeyTakenText,
    TipIdle,
    TipDrawing,
    TipGhost,
    CheatSheet,
    CheatSheetClose,
    WithPen,
    Grip,
    NoShortcut,
}

impl Lang {
    pub fn t(self, key: T) -> &'static str {
        use Lang::*;
        use T::*;
        match (self, key) {
            (En, Draw) => "Draw",
            (Ru, Draw) => "Рисовать",
            (En, GhostMenu) => "Ghost mode (click through)",
            (Ru, GhostMenu) => "Режим призрака (клики насквозь)",
            (En, Whiteboard) => "Whiteboard",
            (Ru, Whiteboard) => "Белая доска",
            (En, DimMenu) => "Dim screen while drawing",
            (Ru, DimMenu) => "Затемнять экран при рисовании",
            (En, DimOff) => "Off",
            (Ru, DimOff) => "Нет",
            (En, DimSlightly) => "Slightly",
            (Ru, DimSlightly) => "Слегка",
            (En, DimMedium) => "Medium",
            (Ru, DimMedium) => "Средне",
            (En, DimStrongly) => "Strongly",
            (Ru, DimStrongly) => "Сильно",
            (En, FadeMenu) => "Fade drawings",
            (Ru, FadeMenu) => "Растворять рисунки",
            (En, FadeOff) => "Off",
            (Ru, FadeOff) => "Нет",
            (En, FadeAfter) => "After {n} s",
            (Ru, FadeAfter) => "Через {n} с",
            (En, HidePalette) => "Hide palette from screen sharing",
            (Ru, HidePalette) => "Прятать палитру из демонстрации экрана",
            (En, SnapshotSaved) => "Screenshot saved and copied",
            (Ru, SnapshotSaved) => "Снимок сохранён и скопирован",
            (En, SnapshotFailed) => "Screenshot failed",
            (Ru, SnapshotFailed) => "Снимок не получился",
            (En, Autostart) => "Start with Windows",
            (Ru, Autostart) => "Запускать вместе с Windows",
            (En, Settings) => "Settings…",
            (Ru, Settings) => "Настройки…",
            (En, Quit) => "Quit",
            (Ru, Quit) => "Выход",
            (En, Running) => "screenpen is running",
            (Ru, Running) => "screenpen запущен",
            (En, AlreadyRunning) => "screenpen is already running",
            (Ru, AlreadyRunning) => "screenpen уже запущен",
            (En, HowToDraw) => "{key} or click the tray icon to draw.",
            (Ru, HowToDraw) => "{key} или клик по значку в трее, чтобы рисовать.",
            (En, HotkeyTaken) => "screenpen: hotkey unavailable",
            (Ru, HotkeyTaken) => "screenpen: сочетание занято",
            (En, HotkeyTakenText) => {
                "Taken by another app: {keys}. Use the tray icon or pick another in Settings."
            }
            (Ru, HotkeyTakenText) => {
                "Занято другим приложением: {keys}. Используй значок в трее или смени в настройках."
            }
            (En, TipIdle) => "screenpen: {key} to draw",
            (Ru, TipIdle) => "screenpen: {key}, чтобы рисовать",
            (En, TipDrawing) => "screenpen: drawing (Esc to stop)",
            (Ru, TipDrawing) => "screenpen: рисование (Esc, чтобы закончить)",
            (En, TipGhost) => "screenpen: ghost mode ({key} to draw)",
            (Ru, TipGhost) => "screenpen: режим призрака ({key}, чтобы рисовать)",
            (En, CheatSheet) => "Shortcuts",
            (Ru, CheatSheet) => "Горячие клавиши",
            (En, CheatSheetClose) => "Press ? or Esc to close",
            (Ru, CheatSheetClose) => "? или Esc, чтобы закрыть",
            (En, WithPen) => {
                "With the pen: Ctrl rectangle, Ctrl+Alt ellipse, Alt arrow, Shift straight/square. Right button erases."
            }
            (Ru, WithPen) => {
                "Пером: Ctrl прямоугольник, Ctrl+Alt эллипс, Alt стрелка, Shift ровно/квадрат. Правая кнопка стирает."
            }
            (En, Grip) => "Drag to move, double-click to rotate",
            (Ru, Grip) => "Тащи, чтобы подвинуть, двойной клик поворачивает",
            (En, NoShortcut) => "no shortcut",
            (Ru, NoShortcut) => "без клавиши",
        }
    }

    pub fn tool(self, tool: ToolKind) -> &'static str {
        use Lang::*;
        use ToolKind::*;
        match (self, tool) {
            (En, Pen) => "Pen",
            (Ru, Pen) => "Перо",
            (En, Highlighter) => "Highlighter",
            (Ru, Highlighter) => "Маркер",
            (En, Line) => "Line",
            (Ru, Line) => "Линия",
            (En, Arrow) => "Arrow",
            (Ru, Arrow) => "Стрелка",
            (En, Rectangle) => "Rectangle",
            (Ru, Rectangle) => "Прямоугольник",
            (En, Ellipse) => "Ellipse",
            (Ru, Ellipse) => "Эллипс",
            (En, Text) => "Text",
            (Ru, Text) => "Текст",
            (En, Badge) => "Numbered badge",
            (Ru, Badge) => "Номер",
            (En, Eraser) => "Eraser",
            (Ru, Eraser) => "Ластик",
            (En, Select) => "Select",
            (Ru, Select) => "Выделение",
            (En, Laser) => "Laser pointer",
            (Ru, Laser) => "Лазерная указка",
        }
    }

    pub fn color(self, index: u8) -> String {
        const EN: [&str; 9] = [
            "Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Pink", "White", "Black",
        ];
        const RU: [&str; 9] = [
            "Красный",
            "Оранжевый",
            "Жёлтый",
            "Зелёный",
            "Синий",
            "Фиолетовый",
            "Розовый",
            "Белый",
            "Чёрный",
        ];
        let names = if self == Lang::Ru { RU } else { EN };
        let prefix = if self == Lang::Ru {
            "Цвет"
        } else {
            "Color"
        };
        match names.get(index as usize) {
            Some(name) => format!("{prefix} {}: {name}", index + 1),
            None => format!("{prefix} {}", index + 1),
        }
    }

    pub fn action(self, action: Action) -> String {
        use Lang::*;
        let fixed =
            |en: &'static str, ru: &'static str| if self == Ru { ru } else { en }.to_string();
        match action {
            Action::Tool(tool) => self.tool(tool).to_string(),
            Action::Color(i) => self.color(i),
            Action::Thinner => fixed("Thinner", "Тоньше"),
            Action::Thicker => fixed("Thicker", "Толще"),
            Action::Clear => fixed("Clear all", "Стереть всё"),
            Action::Undo => fixed("Undo", "Отменить"),
            Action::Redo => fixed("Redo", "Повторить"),
            Action::SelectAll => fixed("Select all", "Выделить всё"),
            Action::BringToFront => fixed("Bring to front", "Наверх"),
            Action::DeleteSelection => fixed("Delete selection", "Удалить выделенное"),
            Action::Escape => fixed("Deselect / stop drawing", "Снять выделение / закончить"),
            Action::Ghost => fixed("Ghost mode", "Режим призрака"),
            Action::Whiteboard => fixed("Whiteboard", "Белая доска"),
            Action::Spotlight => fixed("Spotlight", "Прожектор"),
            Action::Freeze => fixed("Freeze the screen", "Заморозить экран"),
            Action::Snapshot => fixed("Screenshot with drawing", "Снимок экрана с рисунком"),
            Action::Help => fixed("Shortcuts", "Горячие клавиши"),
            Action::ToggleDrawing => {
                fixed("Start / stop drawing", "Включить / выключить рисование")
            }
            Action::ExitGhost => fixed("Leave ghost mode", "Выйти из режима призрака"),
            Action::Quit => fixed("Quit", "Выход"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_both_languages() {
        for b in screenpen_core::Shortcuts::default().bindings {
            assert!(!Lang::En.action(b.action).is_empty());
            assert_ne!(
                Lang::En.action(b.action),
                Lang::Ru.action(b.action),
                "{:?}",
                b.action
            );
        }
    }
}
