//! The settings window (`screenpen --settings`), a separate process on egui: the
//! overlay app owns its message loop, and winit wants to own one too. Every change is
//! saved to the settings file right away and the running app is told to reload it.

use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText};
use screenpen_core::{Action, Chord, Key, Rgba, Shortcuts, TextStyle};

use crate::config::{Settings, default_path, parse_hex};
use crate::i18n::Lang;
use crate::palette::{Item, chord_label};
use crate::state::{DIM_LEVELS, FADE_SECONDS, ToolState, UI_SCALE, step_ui_scale};

pub const TITLE: &str = "screenpen settings";

pub fn run() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([860.0, 700.0])
            .with_min_inner_size([560.0, 420.0]),
        ..Default::default()
    };
    let path = default_path();
    if let Err(e) = eframe::run_native(
        TITLE,
        options,
        Box::new(move |_| Ok(Box::new(SettingsApp::load(path)))),
    ) {
        eprintln!("screenpen settings: {e}");
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Text,
    Shortcuts,
    Palette,
}

struct SettingsApp {
    path: PathBuf,
    settings: Settings,
    state: ToolState,
    tab: Tab,
    /// The action whose new chord is being recorded.
    recording: Option<Action>,
    status: Option<String>,
    /// SCREENPEN_SETTINGS_SHOT=<dir>: save a PNG of each tab and quit (for checking
    /// the layout without a person at the screen).
    shot: Option<(PathBuf, u32)>,
    /// Installed font families, read on first use (it takes a moment).
    families: Option<Vec<String>>,
    font_filter: String,
    /// The text preview and what it was rendered for.
    preview: Option<(String, egui::TextureHandle)>,
    /// Modification time of the file as last read or written.
    loaded: Option<std::time::SystemTime>,
    checked: std::time::Instant,
}

#[derive(Clone, PartialEq)]
struct Edited {
    language: String,
    colors: [Rgba; 9],
    width: f32,
    dim: f32,
    fade: Option<f32>,
    hide_palette: bool,
    shortcuts: Shortcuts,
    items: Vec<String>,
    vertical: bool,
    text_size: f32,
    text_style: TextStyle,
    ui_scale: f32,
}

fn modified(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Picks the English or Russian text.
fn tr(lang: Lang, en: &'static str, ru: &'static str) -> &'static str {
    if lang == Lang::Ru { ru } else { en }
}

impl SettingsApp {
    fn load(path: PathBuf) -> SettingsApp {
        let (settings, error) = Settings::load(&path);
        let mut state = ToolState::default();
        let mut problems = settings.apply(&mut state);
        problems.extend(error);
        let status = (!problems.is_empty()).then(|| problems.join("\n"));
        let loaded = modified(&path);
        SettingsApp {
            path,
            settings,
            state,
            tab: Tab::General,
            recording: None,
            status,
            shot: std::env::var_os("SCREENPEN_SETTINGS_SHOT").map(|dir| (PathBuf::from(dir), 0)),
            loaded,
            families: None,
            font_filter: String::new(),
            preview: None,
            checked: std::time::Instant::now(),
        }
    }

    fn lang(&self) -> Lang {
        self.settings.lang(crate::platform::system_lang())
    }

    /// What the user can change here, to tell which fields a frame changed.
    fn edited(&self) -> Edited {
        Edited {
            language: self.settings.language.clone(),
            colors: self.state.colors,
            width: self.state.width,
            dim: self.state.dim,
            fade: self.state.auto_fade,
            hide_palette: self.settings.hide_palette_from_capture,
            shortcuts: self.state.shortcuts.clone(),
            items: self.settings.palette.items.clone(),
            vertical: self.settings.palette.vertical,
            text_size: self.state.text_size,
            text_style: self.state.text_style.clone(),
            ui_scale: self.state.ui_scale,
        }
    }

    /// Writes only the fields that changed onto a fresh copy of the file, so changes
    /// the running app made meanwhile (width from the wheel, tray toggles, the palette
    /// position) survive.
    fn save(&mut self, before: &Edited) {
        let now = self.edited();
        if &now == before {
            return;
        }
        let (mut disk, error) = Settings::load(&self.path);
        if let Some(e) = error.filter(|_| self.path.exists()) {
            self.status = Some(e);
            return;
        }
        let mut state = ToolState::default();
        disk.apply(&mut state);
        if now.language != before.language {
            disk.language = now.language.clone();
        }
        if now.colors != before.colors {
            state.colors = now.colors;
        }
        if now.width != before.width {
            state.width = now.width;
        }
        if now.dim != before.dim {
            state.dim = now.dim;
        }
        if now.fade != before.fade {
            state.auto_fade = now.fade;
        }
        if now.hide_palette != before.hide_palette {
            disk.hide_palette_from_capture = now.hide_palette;
        }
        if now.shortcuts != before.shortcuts {
            state.shortcuts = now.shortcuts.clone();
        }
        if now.items != before.items {
            disk.palette.items = now.items.clone();
        }
        if now.vertical != before.vertical {
            disk.palette.vertical = now.vertical;
        }
        if now.text_size != before.text_size {
            state.text_size = now.text_size;
        }
        if now.text_style != before.text_style {
            state.text_style = now.text_style.clone();
        }
        if now.ui_scale != before.ui_scale {
            state.ui_scale = now.ui_scale;
        }
        let settings = Settings::capture(
            &state,
            &disk.palette,
            &disk.language,
            disk.hide_palette_from_capture,
        );
        match settings.save(&self.path) {
            Ok(()) => {
                self.settings = settings;
                self.state = state;
                self.status = None;
                self.loaded = modified(&self.path);
                crate::platform::notify_settings_changed();
            }
            Err(e) => self.status = Some(format!("{}: {e}", self.path.display())),
        }
    }

    /// Picks up changes the running app wrote (checked about once a second).
    fn refresh(&mut self) {
        if self.recording.is_some() || self.checked.elapsed() < std::time::Duration::from_secs(1) {
            return;
        }
        self.checked = std::time::Instant::now();
        let stamp = modified(&self.path);
        if stamp != self.loaded {
            let fresh = SettingsApp::load(self.path.clone());
            self.settings = fresh.settings;
            self.state = fresh.state;
            self.status = fresh.status;
            self.loaded = stamp;
        }
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.refresh();
        let before = self.edited();
        self.draw(ui);
        self.save(&before);
        // Keep polling for changes from the running app.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
    }
}

impl SettingsApp {
    fn draw(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        if ui.ctx().zoom_factor() != self.state.ui_scale {
            ui.ctx().set_zoom_factor(self.state.ui_scale);
        }
        self.take_shot(ui);
        if self.recording.is_some() {
            self.record_chord(ui);
        }
        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (tab, en, ru) in [
                    (Tab::General, "General", "Общие"),
                    (Tab::Text, "Text", "Текст"),
                    (Tab::Shortcuts, "Shortcuts", "Горячие клавиши"),
                    (Tab::Palette, "Palette", "Палитра"),
                ] {
                    if ui
                        .selectable_label(self.tab == tab, tr(lang, en, ru))
                        .clicked()
                    {
                        self.tab = tab;
                        self.recording = None;
                    }
                }
            });
        });
        if let Some(status) = &self.status {
            egui::Panel::bottom("status").show(ui, |ui| {
                ui.colored_label(Color32::from_rgb(0xff, 0x9f, 0x0a), status);
            });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                Tab::General => self.general(ui, lang),
                Tab::Text => self.text(ui, lang),
                Tab::Shortcuts => self.shortcuts(ui, lang),
                Tab::Palette => self.palette(ui, lang),
            });
        });
    }
}

impl SettingsApp {
    fn take_shot(&mut self, ui: &mut egui::Ui) {
        let Some((dir, frame)) = &mut self.shot else {
            return;
        };
        *frame += 1;
        let tabs = [Tab::General, Tab::Text, Tab::Shortcuts, Tab::Palette];
        let index = (*frame / 10) as usize;
        ui.ctx().request_repaint();
        let Some(&tab) = tabs.get(index) else {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        self.tab = tab;
        if *frame % 10 == 5 {
            let file = dir.join(format!("settings-{index}.png"));
            let save =
                egui::ScreenshotCallback::new(move |image: std::sync::Arc<egui::ColorImage>| {
                    let [w, h] = image.size;
                    if let Some(mut pixmap) = tiny_skia::Pixmap::new(w as u32, h as u32) {
                        for (dst, src) in pixmap.pixels_mut().iter_mut().zip(&image.pixels) {
                            let [r, g, b, a] = src.to_srgba_unmultiplied();
                            *dst = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
                        }
                        let _ = pixmap.save_png(file);
                    }
                });
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Screenshot(save));
        }
    }
}

// MARK: general

impl SettingsApp {
    fn general(&mut self, ui: &mut egui::Ui, lang: Lang) {
        let mut changed = false;
        egui::Grid::new("general")
            .num_columns(2)
            .spacing([24.0, 12.0])
            .show(ui, |ui| {
                ui.label(tr(lang, "Language", "Язык"));
                egui::ComboBox::from_id_salt("language")
                    .selected_text(match self.settings.language.as_str() {
                        "en" => "English",
                        "ru" => "Русский",
                        _ => tr(lang, "Same as the system", "Как в системе"),
                    })
                    .show_ui(ui, |ui| {
                        for (value, label) in [
                            ("auto", tr(lang, "Same as the system", "Как в системе")),
                            ("en", "English"),
                            ("ru", "Русский"),
                        ] {
                            if ui
                                .selectable_label(self.settings.language == value, label)
                                .clicked()
                            {
                                self.settings.language = value.to_string();
                                changed = true;
                            }
                        }
                    });
                ui.end_row();

                ui.label(tr(lang, "Colors (keys 1-9)", "Цвета (клавиши 1-9)"));
                ui.horizontal_wrapped(|ui| {
                    for color in self.state.colors.iter_mut() {
                        let mut rgb = to_rgb(*color);
                        if ui.color_edit_button_srgb(&mut rgb).changed() {
                            *color = from_rgb(rgb);
                            changed = true;
                        }
                    }
                    if ui.small_button(tr(lang, "Reset", "Сбросить")).clicked() {
                        self.state.colors = Rgba::PALETTE;
                        changed = true;
                    }
                });
                ui.end_row();

                ui.label(tr(lang, "Line width", "Толщина линии"));
                changed |= ui
                    .add(egui::Slider::new(&mut self.state.width, 1.0..=32.0).step_by(1.0))
                    .changed();
                ui.end_row();

                ui.label(tr(
                    lang,
                    "Dim the screen while drawing",
                    "Затемнять экран при рисовании",
                ));
                ui.horizontal(|ui| {
                    let names = [
                        tr(lang, "Off", "Нет"),
                        tr(lang, "Slightly", "Слегка"),
                        tr(lang, "Medium", "Средне"),
                        tr(lang, "Strongly", "Сильно"),
                    ];
                    for ((level, _), name) in DIM_LEVELS.iter().zip(names) {
                        if ui.radio(self.state.dim == *level, name).clicked() {
                            self.state.dim = *level;
                            changed = true;
                        }
                    }
                });
                ui.end_row();

                ui.label(tr(lang, "Fade drawings", "Растворять рисунки"));
                ui.horizontal(|ui| {
                    if ui
                        .radio(self.state.auto_fade.is_none(), tr(lang, "Off", "Нет"))
                        .clicked()
                    {
                        self.state.auto_fade = None;
                        changed = true;
                    }
                    for seconds in FADE_SECONDS {
                        let label = format!("{seconds} {}", tr(lang, "s", "с"));
                        if ui
                            .radio(self.state.auto_fade == Some(seconds), label)
                            .clicked()
                        {
                            self.state.auto_fade = Some(seconds);
                            changed = true;
                        }
                    }
                });
                ui.end_row();

                ui.label(tr(lang, "Screen sharing", "Демонстрация экрана"));
                changed |= ui
                    .checkbox(
                        &mut self.settings.hide_palette_from_capture,
                        tr(
                            lang,
                            "Hide the palette from viewers",
                            "Прятать палитру от зрителей",
                        ),
                    )
                    .changed();
                ui.end_row();

                ui.label(tr(lang, "Interface scale", "Масштаб интерфейса"));
                ui.horizontal(|ui| {
                    let (min, max, _) = UI_SCALE;
                    if ui.small_button("−").clicked() {
                        self.state.ui_scale = step_ui_scale(self.state.ui_scale, false);
                    }
                    let mut percent = (self.state.ui_scale * 100.0).round();
                    let slider = egui::Slider::new(&mut percent, min * 100.0..=max * 100.0).step_by(5.0).suffix("%");
                    if ui.add(slider).changed() {
                        self.state.ui_scale = percent / 100.0;
                    }
                    if ui.small_button("+").clicked() {
                        self.state.ui_scale = step_ui_scale(self.state.ui_scale, true);
                    }
                })
                .response
                .on_hover_text(tr(
                    lang,
                    "Palette, tooltips, cheat sheet and this window. Ctrl+wheel over the palette changes it too.",
                    "Палитра, подсказки, шпаргалка и это окно. Ещё меняется Ctrl+колесом над палитрой.",
                ));
                ui.end_row();
            });
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.path.display().to_string()).weak());
        });
        if changed {}
    }
}

// MARK: text

impl SettingsApp {
    fn text(&mut self, ui: &mut egui::Ui, lang: Lang) {
        let system = tr(lang, "System font", "Системный шрифт");
        egui::Grid::new("text")
            .num_columns(2)
            .spacing([24.0, 12.0])
            .show(ui, |ui| {
                ui.label(tr(lang, "Font", "Шрифт"));
                let current = self.state.text_style.family.clone();
                egui::ComboBox::from_id_salt("font")
                    .selected_text(current.as_deref().unwrap_or(system))
                    .width(260.0)
                    .height(360.0)
                    .show_ui(ui, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.font_filter)
                                .hint_text(tr(lang, "Search…", "Поиск…"))
                                .desired_width(240.0),
                        );
                        if ui.selectable_label(current.is_none(), system).clicked() {
                            self.state.text_style.family = None;
                        }
                        let filter = self.font_filter.to_lowercase();
                        let families = self
                            .families
                            .get_or_insert_with(screenpen_core::text::families);
                        for family in families
                            .iter()
                            .filter(|f| f.to_lowercase().contains(&filter))
                        {
                            if ui
                                .selectable_label(current.as_deref() == Some(family), family)
                                .clicked()
                            {
                                self.state.text_style.family = Some(family.clone());
                            }
                        }
                    });
                ui.end_row();

                ui.label(tr(lang, "Size", "Размер"));
                ui.add(
                    egui::Slider::new(&mut self.state.text_size, 8.0..=200.0)
                        .logarithmic(true)
                        .step_by(1.0)
                        .suffix(" pt"),
                );
                ui.end_row();

                ui.label(tr(lang, "Style", "Начертание"));
                ui.horizontal(|ui| {
                    let style = &mut self.state.text_style;
                    ui.checkbox(&mut style.bold, tr(lang, "Bold", "Жирный"));
                    ui.checkbox(&mut style.italic, tr(lang, "Italic", "Курсив"));
                    ui.checkbox(
                        &mut style.background,
                        tr(lang, "Background plate", "Подложка"),
                    );
                    ui.checkbox(&mut style.outline, tr(lang, "Outline", "Контур"));
                });
                ui.end_row();
            });
        ui.add_space(16.0);
        ui.label(
            RichText::new(tr(
                lang,
                "New text uses these. While typing: Ctrl+wheel, Ctrl+= and Ctrl+- change the size, Ctrl+B and Ctrl+I bold and italic. With text selected, the same keys change it.",
                "Так выглядит новый текст. При наборе Ctrl+колесо, Ctrl+= и Ctrl+- меняют размер, Ctrl+B и Ctrl+I жирный и курсив. Если текст выделен, те же клавиши меняют его.",
            ))
            .weak(),
        );
        ui.add_space(12.0);
        self.preview(ui, lang);
    }

    /// The sample text rendered by the same code that draws annotations.
    fn preview(&mut self, ui: &mut egui::Ui, lang: Lang) {
        let sample = tr(lang, "Screenpen text 123", "Текст screenpen 123");
        let color = self.state.color();
        let key = format!(
            "{sample}|{}|{:?}|{:?}",
            self.state.text_size, self.state.text_style, color
        );
        if self.preview.as_ref().is_none_or(|(k, _)| *k != key) {
            let pixels_per_point = ui.ctx().pixels_per_point();
            let annotation = screenpen_core::Annotation::new(
                screenpen_core::Geometry::Text {
                    origin: screenpen_core::Point::new(12.0, 12.0),
                    string: sample.to_string(),
                    font_size: self.state.text_size,
                    style: self.state.text_style.clone(),
                },
                screenpen_core::Style::new(color, 2.0),
            );
            let bounds = screenpen_core::render::bounds(&annotation)
                .unwrap_or(screenpen_core::Rect::new(0.0, 0.0, 100.0, 40.0));
            let (w, h) = (bounds.right() + 12.0, bounds.bottom() + 12.0);
            let scale = pixels_per_point;
            let Some(mut pixmap) =
                tiny_skia::Pixmap::new((w * scale).ceil() as u32, (h * scale).ceil() as u32)
            else {
                return;
            };
            pixmap.fill(tiny_skia::Color::from_rgba8(0x3a, 0x3a, 0x3c, 255));
            screenpen_core::render::draw(
                &mut pixmap.as_mut(),
                &annotation,
                tiny_skia::Transform::from_scale(scale, scale),
                screenpen_core::render::ChannelOrder::Rgba,
            );
            let image = egui::ColorImage::from_rgba_premultiplied(
                [pixmap.width() as usize, pixmap.height() as usize],
                pixmap.data(),
            );
            let texture =
                ui.ctx()
                    .load_texture("text-preview", image, egui::TextureOptions::LINEAR);
            self.preview = Some((key, texture));
        }
        if let Some((_, texture)) = &self.preview {
            let size = texture.size_vec2() / ui.ctx().pixels_per_point();
            ui.image((texture.id(), size));
        }
    }
}

fn to_rgb(c: Rgba) -> [u8; 3] {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [b(c.red), b(c.green), b(c.blue)]
}

fn from_rgb(rgb: [u8; 3]) -> Rgba {
    parse_hex(&format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])).unwrap_or(Rgba::BLACK)
}

// MARK: shortcuts

impl SettingsApp {
    fn shortcuts(&mut self, ui: &mut egui::Ui, lang: Lang) {
        ui.label(
            RichText::new(tr(
                lang,
                "Click a shortcut and press the new keys. Esc cancels, × turns it off. Keys are physical: P stays P on any layout.",
                "Нажми на сочетание и затем новые клавиши. Esc отменяет, × выключает. Клавиши физические: P остаётся P на любой раскладке.",
            ))
            .weak(),
        );
        ui.add_space(8.0);
        let conflicts: Vec<Action> = self
            .state
            .shortcuts
            .conflicts()
            .into_iter()
            .flat_map(|(a, b)| [a, b])
            .collect();
        let mut changed = false;
        egui::Grid::new("shortcuts").num_columns(3).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
            for binding in self.state.shortcuts.bindings.clone() {
                let action = binding.action;
                let mut name = RichText::new(lang.action(action));
                if action.is_global() {
                    name = name.strong();
                }
                ui.label(name).on_hover_text(if action.is_global() {
                    tr(lang, "Works everywhere, even when screenpen isn't drawing", "Работает везде, даже когда screenpen не рисует")
                } else {
                    tr(lang, "Works while drawing", "Работает во время рисования")
                });
                let recording = self.recording == Some(action);
                let text = if recording {
                    RichText::new(tr(lang, "Press keys…", "Нажми клавиши…")).italics()
                } else {
                    match binding.chord {
                        Some(c) => {
                            let t = RichText::new(chord_label(c)).monospace();
                            if conflicts.contains(&action) { t.color(Color32::from_rgb(0xff, 0x45, 0x3a)) } else { t }
                        }
                        None => RichText::new(tr(lang, "off", "выкл")).weak(),
                    }
                };
                let button = ui.add(egui::Button::new(text).min_size(egui::vec2(170.0, 0.0)));
                if button.clicked() {
                    self.recording = if recording { None } else { Some(action) };
                }
                if conflicts.contains(&action) {
                    button.on_hover_text(tr(lang, "Also used by another action", "Уже занято другим действием"));
                } else if action.is_global() && binding.chord.is_some_and(|c| !c.usable_globally()) {
                    button.on_hover_text(tr(
                        lang,
                        "A global shortcut needs Ctrl or Alt, or it would steal normal typing",
                        "Глобальному сочетанию нужен Ctrl или Alt, иначе оно перехватит обычный ввод",
                    ));
                }
                if ui.add_enabled(binding.chord.is_some(), egui::Button::new("×").small()).clicked() {
                    self.state.shortcuts.set(action, None);
                    changed = true;
                }
                ui.end_row();
            }
        });
        ui.add_space(12.0);
        if ui
            .button(tr(lang, "Reset all shortcuts", "Сбросить все сочетания"))
            .clicked()
        {
            self.state.shortcuts = Shortcuts::default();
            self.recording = None;
            changed = true;
        }
        if changed {}
    }

    /// Takes the next key press as the chord for the action being recorded.
    fn record_chord(&mut self, ui: &mut egui::Ui) {
        let Some(action) = self.recording else { return };
        let events = ui.input(|i| i.events.clone());
        for event in events {
            let egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                continue;
            };
            let Some(key) = from_egui(physical_key.unwrap_or(key)) else {
                continue;
            };
            let chord = Chord {
                ctrl: modifiers.ctrl,
                alt: modifiers.alt,
                shift: modifiers.shift,
                key,
            };
            self.recording = None;
            if chord == Chord::key(Key::Escape) && action != Action::Escape {
                return;
            }
            self.state.shortcuts.set(action, Some(chord));
            return;
        }
    }
}

/// egui keys to physical keys named after the US layout.
fn from_egui(key: egui::Key) -> Option<Key> {
    use egui::Key as E;
    let name = key.name();
    Some(match key {
        E::Escape => Key::Escape,
        E::Enter => Key::Enter,
        E::Space => Key::Space,
        E::Tab => Key::Tab,
        E::Backspace => Key::Backspace,
        E::Delete => Key::Delete,
        E::Insert => Key::Insert,
        E::Home => Key::Home,
        E::End => Key::End,
        E::PageUp => Key::PageUp,
        E::PageDown => Key::PageDown,
        E::ArrowLeft => Key::Left,
        E::ArrowRight => Key::Right,
        E::ArrowUp => Key::Up,
        E::ArrowDown => Key::Down,
        E::OpenBracket => Key::Char('['),
        E::CloseBracket => Key::Char(']'),
        E::Slash | E::Questionmark => Key::Char('/'),
        E::Backslash => Key::Char('\\'),
        E::Comma => Key::Char(','),
        E::Period => Key::Char('.'),
        E::Minus => Key::Char('-'),
        E::Equals | E::Plus => Key::Char('='),
        E::Semicolon | E::Colon => Key::Char(';'),
        E::Quote => Key::Char('\''),
        E::Backtick => Key::Char('`'),
        _ if name.len() == 1 => Key::Char(name.chars().next()?.to_ascii_uppercase()),
        _ => match name.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
            Some(n) if (1..=24).contains(&n) => Key::F(n),
            _ => return None,
        },
    })
}

// MARK: palette

impl SettingsApp {
    fn palette(&mut self, ui: &mut egui::Ui, lang: Lang) {
        let mut items = self.settings.palette_items();
        let mut changed = false;
        ui.label(
            RichText::new(tr(
                lang,
                "Drag to reorder. The palette updates as you go.",
                "Перетаскивай, чтобы поменять порядок. Палитра обновляется сразу.",
            ))
            .weak(),
        );
        ui.add_space(8.0);
        ui.columns(2, |cols| {
            cols[0].heading(tr(lang, "On the palette", "На палитре"));
            let mut moved: Option<(usize, usize)> = None;
            let mut removed: Option<usize> = None;
            let (_, dropped) = cols[0].dnd_drop_zone::<usize, ()>(egui::Frame::default(), |ui| {
                for (i, item) in items.iter().enumerate() {
                    let id = ui.make_persistent_id(("palette-item", i));
                    let row = ui.horizontal(|ui| {
                        ui.dnd_drag_source(id, i, |ui| {
                            ui.label(item.label(lang));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("×").clicked() {
                                removed = Some(i);
                            }
                        });
                    });
                    if let Some(from) = row.response.dnd_hover_payload::<usize>() {
                        let rect = row.response.rect;
                        let y = if *from > i { rect.top() } else { rect.bottom() };
                        ui.painter().hline(
                            rect.x_range(),
                            y,
                            egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                        );
                    }
                    if let Some(from) = row.response.dnd_release_payload::<usize>() {
                        moved = Some((*from, i));
                    }
                }
            });
            let _ = dropped;
            if let Some((from, to)) = moved
                && from != to
            {
                let item = items.remove(from);
                items.insert(to, item);
                changed = true;
            }
            if let Some(i) = removed {
                items.remove(i);
                changed = true;
            }

            cols[1].heading(tr(lang, "Available", "Можно добавить"));
            for item in Item::all() {
                if items.contains(&item) {
                    continue;
                }
                cols[1].horizontal(|ui| {
                    if ui.small_button("+").clicked() {
                        items.push(item);
                        changed = true;
                    }
                    ui.label(item.label(lang));
                });
            }
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            changed |= ui
                .checkbox(
                    &mut self.settings.palette.vertical,
                    tr(lang, "Vertical", "Вертикально"),
                )
                .changed();
            if ui
                .button(tr(lang, "Default buttons", "Кнопки по умолчанию"))
                .clicked()
            {
                items = Item::defaults();
                changed = true;
            }
        });
        if changed {
            if items.is_empty() {
                items = Item::defaults();
            }
            self.settings.palette.items = items.iter().map(|i| i.name()).collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::hex;

    #[test]
    fn egui_keys_map_to_physical_keys() {
        assert_eq!(from_egui(egui::Key::P), Some(Key::Char('P')));
        assert_eq!(from_egui(egui::Key::Num7), Some(Key::Char('7')));
        assert_eq!(from_egui(egui::Key::F9), Some(Key::F(9)));
        assert_eq!(from_egui(egui::Key::OpenBracket), Some(Key::Char('[')));
        assert_eq!(from_egui(egui::Key::Escape), Some(Key::Escape));
    }

    #[test]
    fn color_conversion_round_trips() {
        for c in Rgba::PALETTE {
            assert_eq!(hex(from_rgb(to_rgb(c))), hex(c));
        }
    }

    #[test]
    fn saving_keeps_what_the_app_changed_meanwhile() {
        let dir = std::env::temp_dir().join(format!("screenpen-merge-{}", std::process::id()));
        let path = dir.join("settings.toml");
        Settings::default().save(&path).unwrap();
        let mut window = SettingsApp::load(path.clone());
        // The running app dims the screen and rotates the palette...
        let mut app_side = Settings::load(&path).0;
        app_side.dim = 0.45;
        app_side.palette.vertical = true;
        app_side.save(&path).unwrap();
        // ...while the settings window, opened earlier, changes the width.
        let before = window.edited();
        window.state.width = 9.0;
        window.save(&before);
        let merged = Settings::load(&path).0;
        assert_eq!(merged.width, 9.0);
        assert_eq!(merged.dim, 0.45);
        assert!(merged.palette.vertical);
        std::fs::remove_dir_all(dir).ok();
    }
}
