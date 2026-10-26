# screenpen: план

Rust-порт [ScreenInk](https://github.com/volyx/ScreenInk) (порт согласован с автором):
overlay для рисования поверх экрана на презентациях, демо и записи экрана.
Формат плана по мотивам [obra/superpowers](https://github.com/obra/superpowers) (MIT).

## Решения

| Тема | Решение |
|---|---|
| Цель | OSS под MIT для себя, подпись exe и нотаризация позже |
| Платформы | Windows 11 x64, затем macOS arm64, затем Linux (X11/Wayland) |
| Окна | свой platform-слой: `windows` (windows-rs), позже `objc2` |
| Рендер | CPU, `tiny-skia`, перерисовка только dirty-областей |
| Вывод | `UpdateLayeredWindow` с premultiplied BGRA |
| Текст | `cosmic-text`, простой ввод без IME |
| Ввод | `WM_POINTER` с первого дня, давление пера отдельной фазой |
| Палитра | отдельное owned-окно, рисуем сами в tiny-skia |
| Захват | палитру можно скрыть из трансляции (`WDA_EXCLUDEFROMCAPTURE`), переключатель в трее |
| Хоткеи | Ctrl+Alt+Shift+A рисование, Ctrl+Alt+Shift+G выход из ghost, G вход |
| Фокус | overlay забирает фокус, PgUp/PgDn/стрелки пересылает в прошлое окно |
| Модификаторы пера | Ctrl прямоугольник, Ctrl+Alt эллипс, Alt стрелка, Shift ограничение |
| Настройки | TOML в `%APPDATA%\screenpen`, автозапуск через `HKCU\...\Run` |

Из markeron (MIT) берём: `WS_EX_TOOLWINDOW` (иначе видео в браузере под окном
чёрное), окно на 1px ниже монитора (иначе Win11 считает его fullscreen и ломает
таскбар), позиция и размер одним `SetWindowPos`. Не берём: WebView, авто-ghost при
потере фокуса, overlay только на одном мониторе.

## Структура

- `crates/screenpen-core`: модель, геометрия, undo, растеризация в tiny-skia.
  Без зависимостей от ОС, всё покрыто тестами, собирается и тестируется на Linux.
- `crates/screenpen`: приложение. `src/platform/<os>/` окна, ввод, хоткеи, трей.
- Поток данных как в ScreenInk: ввод → инструмент → мутация `Document` (снимок
  в undo) → перерисовка. Инструменты трогают только bitmap живого штриха.
- Координаты: физические пиксели монитора, начало сверху слева.

## Фазы

Статус ведём здесь же, после каждой фазы.

**Фаза 0. Спайк overlay на Windows.** Снимает главный риск стека.
- Окно на каждый монитор: layered, topmost, toolwindow, per-monitor DPI v2.
- Глобальный хоткей, рисование мышью полилинией через tiny-skia, Esc, ghost mode.
- Пересылка PgUp/PgDn/стрелок в предыдущее окно.
- Тестовое owned-окно палитры с переключением `WDA_EXCLUDEFROMCAPTURE` по F9.
- *Готово, когда* на твоей Win11 пройден чек-лист ниже.

**Фаза 1. Порт core.**
- Annotation, Style, Document (+undo), StrokeBuilder, PressureFilter, StrokeOutline,
  Tools, Shortcuts, рендер аннотаций в tiny-skia.
- Тесты ScreenInk переносятся один в один, плюс self-test: рендер всех видов
  аннотаций в PNG (CI на Linux).
- *Готово, когда* `cargo test` зелёный, PNG self-test совпадают с ожиданием на глаз.

**Фаза 2. Инструменты на Windows.**
- Bitmap-кэш живого штриха, все инструменты, выделение, ластик, текст.
- Модификаторы пера, ПКМ ластик, Ctrl+колесо толщина, W доска, затемнение,
  автозатухание.
- *Готово, когда* все инструменты работают на всех мониторах, undo после любого действия.

**Фаза 3. Палитра, трей, настройки.**
- Палитра (инструменты, 9 цветов, толщина), перетаскивание с запоминанием позиции.
- Трей: включить, ghost, затемнение, автозатухание, скрыть палитру из захвата,
  автозапуск, выход. Настройки в TOML.

**Фаза 4. Ghost mode и полировка Windows.**
- Подключение/отключение мониторов без потери рисунков, смешанный DPI.
- Поверх fullscreen: детект чужого окна без `WS_CAPTION` во весь монитор.
- Релиз: zip с exe из CI (без подписи, SmartScreen предупредит).

**Фаза 5. macOS.** NSPanel non-activating, уровень screenSaver, все Spaces,
вывод через CALayer contents, Carbon hotkey.

**Фаза 6. Linux.** X11 и Wayland (layer-shell где есть).

**v2:** spotlight, лупа, freeze, снапшоты, экспорт PNG/PDF, давление пера, лазер.

## Чек-лист фазы 0 (руками на Win11)

1. Ctrl+Alt+Shift+A показывает overlay на всех мониторах, рисуется мышью, Esc прячет.
2. Поверх браузера в F11 и PowerPoint Slide Show (в том числе на втором мониторе).
3. В Slide Show PgDn/стрелки листают слайды, рисунок остаётся.
4. Рисунок виден в демонстрации экрана Teams/Zoom/OBS; палитра после F9 не видна.
5. G включает ghost: клики проходят в окно под overlay. Ctrl+Alt+Shift+G возвращает рисование.
6. Мониторы с разным масштабом (100% и 150%): линия идёт ровно под курсором.
7. Видео в браузере под overlay не чернеет, таскбар не меняет вид.

## Проверки

- `cargo test --workspace` и `cargo clippy --workspace -- -D warnings` (Linux).
- `cargo check --target x86_64-pc-windows-msvc -p screenpen` на VM.
- CI: Linux тесты + self-test, Windows сборка exe в артефакт.

## Откат

Если фаза 0 покажет, что layered-окно не перекрывает Slide Show или не попадает
в захват, пробуем DirectComposition-окно (`WS_EX_NOREDIRECTIONBITMAP`) с тем же
tiny-skia-буфером. Core от этого не меняется.

## Статус

2026-10-26: решения приняты, начата фаза 0.
