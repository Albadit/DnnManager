# GPUI 0.2.2 - layout, styling, text and drawing elements

Source read: `gpui-0.2.2/src` (paths below are relative to it) and `gpui-macros-0.2.2/src/styles.rs`
(written `macros:styles.rs:N`). Everything cites file:line in 0.2.2. "UNVERIFIED" = inferred, not proven
by reading the code path end to end.

## 1. The Style model

`Styled` (styled.rs:22) has one required method `style(&mut self) -> &mut StyleRefinement`; every helper
writes `Some(..)` into that refinement. The element's final `Style` is `Style::default()` refined by
base style, then focus/hover/active/group refinements (elements/div.rs:2480-2503).

### Style defaults - `impl Default for Style` (style.rs:731-781)
| field | default | note |
|---|---|---|
| display | `Display::Block` (style.rs:734) | but `Display::default()` is `Flex` (style.rs:1091-1097): a hand-built `Style { ..Default::default() }` is block |
| visibility | Visible | |
| overflow | Visible/Visible | |
| position | `Relative` (style.rs:743) | |
| inset | all `auto` | |
| size / min_size / max_size | `auto` | |
| margin, padding, border_widths | zero | |
| gap | zero | |
| align_items/self/content, justify_content | `None` (taffy default) | |
| flex_direction | `Row` (style.rs:758) | |
| flex_wrap | `NoWrap` | |
| flex_grow / flex_shrink / flex_basis | `0.0` / `1.0` / `Auto` (style.rs:760-762) | CSS-like: items shrink by default |
| background, border_color | `None` | |
| corner_radii | 0, box_shadow empty, opacity `None`, mouse_cursor `None` | |
| text | empty `TextStyleRefinement` | |

### What each Styled method sets (grouped)
- **Display**: `block` / `flex` / `grid` / `hidden` (= `Display::None`, removes from layout) (styled.rs:37-61).
- **Flex**: `flex_col`, `flex_col_reverse`, `flex_row`, `flex_row_reverse` (styled.rs:137-162);
  `flex_1` = grow 1, shrink 1, basis `relative(0.)` (styled.rs:165-171); `flex_auto` = 1/1/auto;
  `flex_initial` = 0/1/auto; `flex_none` = grow 0, shrink 0 (basis untouched) (styled.rs:174-197);
  `flex_basis(len)`, `flex_grow()` (=1), `flex_shrink()` (=1), `flex_shrink_0()`, `flex_wrap`,
  `flex_wrap_reverse`, `flex_nowrap` (styled.rs:200-246).
- **Alignment**: `items_start/end/center/baseline` (styled.rs:249-273, `items_start` = `FlexStart`),
  `justify_start/end/center/between/around` (styled.rs:277-310), `content_normal/center/start/end/
  between/around/evenly/stretch` (= align_content, styled.rs:314-369). No `justify_evenly`, no `self_*`
  helper - set `style().align_self` directly.
- **Sizing** (macro, `macros:styles.rs:898-969`): `w`, `h`, `size` (both axes), `min_w`, `min_h`,
  `max_w`, `max_h` take `impl Into<Length>` (auto allowed); `gap`, `gap_x`, `gap_y` take
  `DefiniteLength` (no auto). Each prefix also gets suffix methods: `_0`, `_0p5`(0.125rem) ... `_128`
  (32rem) in 0.25rem steps, `_auto`, `_px` (1px), `_full` (100%), fractions `_1_2 _1_3 _2_3 _1_4 _3_4
  _1_5.._4_5 _1_6 _5_6 _1_12` (macros:styles.rs:971-1203), and `_neg` variants for every non-auto
  suffix (macros:styles.rs:623-637). So `w_4()` = 1rem = 16px at default rem.
- **Spacing**: margin `m, mt, mb, my, mx, ml, mr` (auto allowed, macros:styles.rs:754-804); padding
  `p, pt, pb, px, py, pl, pr` (DefiniteLength, no auto, macros:styles.rs:806-856). Same suffixes.
- **Position**: `relative()`, `absolute()` (macros:styles.rs:111-123); inset `inset, top, bottom,
  left, right` with the same suffixes (macros:styles.rs:858-896).
- **Overflow**: `overflow_hidden`, `overflow_x_hidden`, `overflow_y_hidden` (macros:styles.rs:135-155).
  Scrolling (`overflow_scroll`, `overflow_x_scroll`, `overflow_y_scroll`) is only on
  `StatefulInteractiveElement` (needs `.id()`) (elements/div.rs:1049-1066).
- **Borders**: `border_color(c)` (macros:styles.rs:375); widths `border, border_t/b/r/l/x/y` with
  suffixes `_0.._12, _16, _20, _24, _32` - these are **px**, not rems (macros:styles.rs:1323-1460);
  `border_dashed()` (styled.rs:382). **Radius**: `rounded, rounded_t/b/r/l/tl/tr/bl/br` with
  `_none, _xs(0.125rem), _sm(0.25rem), _md(0.375rem), _lg(0.5rem), _xl(0.75rem), _2xl(1rem), _3xl(1.5rem),
  _full(9999px)` (macros:styles.rs:1206-1321).
- **Background**: `bg(impl Into<Fill>)` (styled.rs:372); `Fill` has only `Fill::Color(Background)`
  (style.rs:813-816); `From<Hsla|Rgba|Background>` (style.rs:836-852). `Background` is solid,
  `linear_gradient(angle_deg, from, to)` (2 stops only, color.rs:765-779), `pattern_slash(color, width,
  interval)` (color.rs:737-747), `solid_background` (color.rs:751); `.color_space(ColorSpace::Oklab)`
  for gradient interpolation (color.rs:814, 670-675). Angle 0 = to top, clockwise (color.rs:760-763).
- **Shadows**: `shadow(Vec<BoxShadow>)`, `shadow_none`, presets `shadow_2xs/xs/sm/md/lg/xl/2xl`
  (macros:styles.rs:396-560). `BoxShadow { color, offset, blur_radius, spread_radius }` (style.rs:308-317).
- **Opacity**: `opacity(f32)` (styled.rs:652); multiplies into `element_opacity` for the subtree
  (window.rs:2433-2447).
- **Visibility**: `visible()`, `invisible()` (macros:styles.rs:56-68). Hidden keeps layout space; div
  paint returns before painting itself/children (elements/div.rs:1809-1811).
- **Cursor**: `cursor(CursorStyle)`, `cursor_default/pointer/text/move/not_allowed/context_menu/
  crosshair/vertical_text/alias/copy/no_drop/grab/grabbing/ew_resize/ns_resize/nesw_resize/nwse_resize/
  col_resize/row_resize/n_resize/e_resize/s_resize/w_resize` (macros:styles.rs:164-330); oddity:
  `cursor_none(cursor)` takes an argument (macros:styles.rs:332). A cursor style forces a hitbox
  (elements/div.rs:1693-1700) and is applied in paint via `set_cursor_style` (elements/div.rs:1838-1840).
- **Z-order / occlusion**: there is no z-index in `Style`. Paint order is tree order; overlays use
  `deferred(..).with_priority(n)` (elements/deferred.rs:25). Mouse occlusion is `occlude()` /
  `block_mouse_except_scroll()` on interactive elements (elements/div.rs:997, 1013).
- **Grid** (yes, supported): `grid()`, `grid_cols(u16)`, `grid_rows(u16)`, `col_start/end(i16)`,
  `col_start_auto/col_end_auto`, `col_span(u16)`, `col_span_full`, same for rows (styled.rs:658-752).
  Only equal tracks: `grid_cols(n)` becomes `repeat(n, minmax(0, 1fr))` (taffy.rs:308-314, 339-340);
  no custom templates, no named areas. `col_span_full` = lines 1..-1 (styled.rs:705-709). Example:
  examples/grid_layout.rs:24-60.
- **Debug**: `debug()`, `debug_below()` paint red outlines, debug builds only (styled.rs:755-766,
  style.rs:611-618).

### Text props and inheritance
Text helpers write `style.text` (a `TextStyleRefinement`, styled.rs:388-392): `text_color`, `font_weight`,
`text_bg`, `text_size`, `text_xs(0.75rem) / sm(0.875) / base(1) / lg(1.125) / xl / 2xl / 3xl`
(styled.rs:396-493), `italic`, `not_italic`, `underline`, `line_through`, `text_decoration_*`
(styled.rs:496-612), `font_family`, `font(Font)`, `line_height` (styled.rs:616-649), `whitespace_normal/
nowrap`, `text_ellipsis`, `text_overflow`, `text_align/left/center/right`, `truncate`, `line_clamp`
(styled.rs:65-134).

Cascade: div pushes `style.text` with `window.with_text_style` in request_layout, prepaint and paint
(elements/div.rs:1334, 1672, 1823). `window.text_style()` = `TextStyle::default()` refined by every
refinement on the stack (window.rs:1440-1446). Therefore **every** `TextStyle` field inherits, not only
color/font: `white_space`, `text_overflow`, `line_clamp`, `text_align`, `background_color`, underline
(style.rs:354-399). A `.truncate()` or `.line_clamp(2)` on a container applies to all descendant text.
`TextStyle` defaults (style.rs:401-422): color `black()`, family `".SystemUIFont"`, size `rems(1.)`,
line_height `phi()` = `relative(1.618034)` x font size (geometry.rs:3572-3574), weight normal,
white_space `Normal`, text_overflow `None`, align `Left`, line_clamp `None`.

## 2. Taffy mapping

- One `TaffyTree` per window, rounding enabled (taffy.rs:39-46).
- Conversion happens in **device pixels**: every px/rem length is multiplied by `scale_factor`
  (taffy.rs:356-463); available space is scaled up (taffy.rs:185-197) and results divided back
  (taffy.rs:239-249). With rounding on, bounds snap to whole device pixels (at 125% DPI a logical px
  is 0.8 device px, so logical bounds may be fractional).
- Rems -> px with the current `window.rem_size()` (default 16px, window.rs:1215; overridable per
  subtree with `with_rem_size`, window.rs:1851-1866; global `set_rem_size`, window.rs:1830).
- `DefiniteLength::Fraction(f)` (`relative(f)`, geometry.rs:3567) -> taffy `percent(f)` (taffy.rs:406-408,
  445). Percent against an indefinite parent size follows taffy rules (treated as auto) - UNVERIFIED in
  taffy-0.9.0 source.
- `Length::Auto` -> taffy auto (taffy.rs:371-391). Only margin/inset/size/min/max accept auto.
- `request_layout(style, children)` -> `new_leaf` / `new_with_children` (taffy.rs:55-76, window.rs:3217).
- `request_measured_layout(style, measure)` registers a leaf with a closure `FnMut(known_dimensions:
  Size<Option<Pixels>>, available_space: Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>`
  (taffy.rs:78-102, window.rs:3246). Taffy may call it several times per layout (min-content,
  max-content, definite passes) - measure functions must be cheap or cache (text does, see 3).
- "Definite width" for a measured leaf = `known_dimensions.width` is `Some` (style sets the size, or
  flex stretches it) or `available_space.width` is `Definite`. `MinContent`/`MaxContent` passes are not
  definite (text.rs:347-354). A flex item's automatic min size is its content size unless overflow is
  `Hidden`/`Scroll` (then 0) (style.rs:1168-1182) - so a shrinking flex child with text needs
  `overflow_hidden()` or `min_w_0()`.
- `AnyElement::layout_as_root(available_space, window, cx) -> Size<Pixels>` (element.rs:619) lays out a
  detached element; `prepaint_at` / `prepaint_as_root` (element.rs:630, 641). Lists use this per item.

## 3. Text

**Plain strings**: `&'static str`, `String`, `SharedString` are elements (text.rs:19-142); each builds a
`TextLayout` with the inherited `window.text_style()`. The text node uses `Style::default()`
(text.rs:343) - it cannot be styled itself; style its parent.

**Measure (text.rs:343-433)**:
- wrap width = known width, else definite available width, only when `white_space == Normal`
  (text.rs:347-354).
- truncation width = same width x `line_clamp` lines when `text_overflow` is set (text.rs:356-371).
- Cached result reused when size known and wrap width unchanged (text.rs:373-378) (the known
  "first measure kept" behaviour).
- `truncate_line` runs *before* shaping (text.rs:381-390), then `shape_text(text, font_size, runs,
  wrap_width, line_clamp)` (text.rs:393-401). Height = sum of line heights; width = max line width, ceil
  (text.rs:415-420).
- Line height in the text element: `line_height.to_pixels(font_size, rem)` - not rounded (text.rs:333-335);
  `window.line_height()` rounds (window.rs:1868-1870, style.rs:470-472).
- Paint: background then glyphs per line with `text_align` against the element bounds (text.rs:445-480;
  alignment math text_system/line.rs:586-589).

**StyledText** (text.rs:148): `StyledText::new(text)`, `.with_highlights(iter<(Range<usize>,
HighlightStyle)>)` (resolved against the inherited style at layout, text.rs:188-206, 266-270),
`.with_default_highlights(&TextStyle, highlights)` (text.rs:173), `.with_runs(Vec<TextRun>)` - panics
"invalid text run" unless run lengths sum to the byte length (text.rs:236-244). Ranges are UTF-8 byte
ranges on char boundaries (debug_assert, text.rs:200-201). `.layout() -> &TextLayout` (text.rs:167).
**TextLayout** queries - `index_for_position(Point) -> Result<usize,usize>`, `position_for_index`,
`line_layout_for_index`, `bounds`, `line_height`, `len`, `text`, `wrapped_text` (text.rs:483-622) -
panic if called before measurement/prepaint (text.rs:485-490, 579).

**TextRun** `{ len (bytes), font, color, background_color, underline, strikethrough }`
(text_system.rs:733-746); `TextStyle::to_run(len)` (style.rs:475). **HighlightStyle** `{ color,
font_weight, font_style, background_color, underline, strikethrough, fade_out }` (style.rs:496-517);
`From<Hsla|Rgba|FontWeight|FontStyle|TextStyle>`; colors blend (style.rs:884-910);
`combine_highlights(a, b)` merges overlapping ranges (style.rs:950). No per-run font size.

**InteractiveText** (text.rs:626): `InteractiveText::new(id, styled_text)`, `.on_click(ranges,
Fn(range_ix, &mut Window, &mut App))` - fires only if mouse-down and mouse-up are in the same range
(text.rs:667-682), shows `PointingHand` over clickable ranges (text.rs:783); `.on_hover(Fn(Option<usize>,
MouseMoveEvent, ..))`; `.tooltip(Fn(char_ix, ..) -> Option<AnyView>)` (text.rs:686-701).

**Wrapping** (`LineWrapper::wrap_line`, text_system/line_wrapper.rs:33-129): breaks at word boundaries;
word chars = ASCII alnum, Latin-1/Extended-A/B, Cyrillic, and `- _ . ' $ % @ # ^ ~ , = :`
(line_wrapper.rs:168-190) - so `C:\path\to\file` has no break points except `\`; a too-long word
breaks mid-word (line_wrapper.rs:112-115). Wrapped continuation lines get the first line's indent (max
256 spaces) (line_wrapper.rs:101-119).

**Truncation** (`truncate_line(line, width, suffix, &mut runs) -> SharedString`, line_wrapper.rs:132-164):
sums cached per-char advances of the *base* font (line_wrapper.rs:193-224) - bold runs, fallback fonts
and kerning are ignored, so the cut point is approximate. Cuts when `width.floor() > truncate_width`,
appends the suffix and trims runs (line_wrapper.rs:227-238). `ELLIPSIS` = `"…"` (styled.rs:13);
`TextOverflow::Truncate(SharedString)` allows any suffix, e.g. `""` to clip (style.rs:331-335,
examples/text_wrapper.rs:80). With `line_clamp(n)` the budget is `width * n` over the whole string
(text.rs:359-361) - word wrapping can make the visible result differ.

**line_clamp** (styled.rs:129-133 also sets `overflow_hidden`): passed to `shape_text`, which limits
*wrap* boundaries (`max_wrap_lines - wrapped_lines`, text_system.rs:422, 479-486); hard `\n` lines are
always produced - UNVERIFIED whether `line_clamp(1)` with embedded newlines shows only one line
(the clip from overflow_hidden still hides overflow if the parent height is bounded).

**Fonts / fallbacks**: GPUI resolves `Font` -> `FontId`; on failure walks `.ZedMono, .ZedSans,
Helvetica, Segoe UI, Ubuntu, Adwaita Sans, Cantarell, Noto Sans, DejaVu Sans, Arial` and panics if none
load (text_system.rs:73-85, 150-168). On Windows `.SystemUIFont` = the font from
`SPI_GETICONTITLELOGFONT` (fallback "Segoe UI") (platform/windows/direct_write.rs:462-470, 1819-1840);
an unknown family logs an error and uses the system UI font (direct_write.rs:483-492; it panics under
`test-support`, direct_write.rs:485-487). Per-glyph fallback: `Font.fallbacks: Option<FontFallbacks>`
(`FontFallbacks::from_fonts(vec![..])`, text_system/font_fallbacks.rs:18) builds a DirectWrite fallback
mapping those fonts' unicode ranges, then the system fallback (direct_write.rs:323-377). Custom fonts:
`cx.text_system().add_fonts(Vec<Cow<'static,[u8]>>)` (text_system.rs:104). `font(family)` builder with
`.bold()`, `.italic()` (text_system.rs:807-829); `FontWeight::THIN..BLACK` (text_system.rs:664-680).

**Measuring text yourself** - `window.text_system() -> &Arc<WindowTextSystem>` (window.rs:1435):
```rust
let style = window.text_style();
let font_size = style.font_size.to_pixels(window.rem_size());
let run = style.to_run(text.len());
let line: ShapedLine = window.text_system().shape_line(text.into(), font_size, &[run], None);
let width = line.width;                      // Deref to LineLayout { width, ascent, descent, .. }
line.paint(origin, window.line_height(), window, cx)?; // in paint phase (canvas)
```
Signatures: `shape_line(&self, text: SharedString, font_size: Pixels, runs: &[TextRun], force_width:
Option<Pixels>) -> ShapedLine` - debug-asserts no `\n` (text_system.rs:365-404); `shape_text(text,
font_size, runs, wrap_width: Option<Pixels>, line_clamp: Option<usize>) -> Result<SmallVec<[WrappedLine;
1]>>` (text_system.rs:409-525); `layout_line(&str, font_size, runs, force_width) -> Arc<LineLayout>`
(text_system.rs:535); `LineLayout::x_for_index`, `index_for_x`, `closest_index_for_x`
(text_system/line_layout.rs:58-105). Metrics: `ascent/descent/em_width/ch_width/baseline_offset(font_id,
size, line_height)` with `resolve_font(&Font)` (text_system.rs:150-275); a `LineWrapperHandle` via
`cx.text_system().line_wrapper(font, size)` (text_system.rs:292).

## 4. Drawing elements

**canvas** `canvas(prepaint: FnOnce(Bounds, &mut Window, &mut App) -> T, paint: FnOnce(Bounds, T,
&mut Window, &mut App))` (elements/canvas.rs:10-19). Styled (size it - it has no children, so auto size
is 0), paints its own bg/border around the paint closure (canvas.rs:84-87). No id, no hitbox, no mouse
handlers - put handlers on a wrapping div (examples/painting.rs:355-410). Primitives (paint phase only):
`paint_quad(quad(bounds, radii, bg, border_widths, border_color, BorderStyle))` (window.rs:2839),
`paint_path(Path, impl Into<Background>)` (window.rs:2860), `paint_shadows` (window.rs:2806),
`paint_svg`, `paint_image` (window.rs:3065, 3129), `with_content_mask` (window.rs:2384), `paint_layer`
(window.rs:2782). Paths: `PathBuilder::fill()` / `::stroke(px(w))`, `.dash_array(&[..])`, `move_to,
line_to, curve_to, cubic_bezier_to, arc_to, add_polygon, close, translate/scale/rotate, build() ->
Result<Path<Pixels>>` (path_builder.rs:88-244; lyon tessellation).

**svg** `svg().path("icons/x.svg")` (elements/svg.rs:18-31). The path goes through the app's
`AssetSource`, not the file system; a missing asset paints nothing silently (svg_renderer.rs:63-66).
Colour = **the svg element's own** `text_color` - `style.text.color` of its own refinement, not the
inherited text colour (svg.rs:110); no `text_color` => nothing painted. Rendered as an alpha mask
(monochrome; original colours lost) (svg_renderer.rs:70-80). Raster at 2x (`SMOOTH_SVG_SCALE_FACTOR`,
svg_renderer.rs:9) scaled by **width only**, aspect kept (svg_renderer.rs:86-95), centred in bounds
(window.rs:3098-3112) - give it a box matching the SVG's aspect. `with_transformation(Transformation::
rotate/scale/translate)` affects rendering only, not layout/hitbox (svg.rs:33-38, 148-221). No
`external_path` in 0.2.2. For a full-colour SVG use `img("x.svg")` (img supports svg, img.rs:209-215).

**img** `img(source)` (elements/img.rs:198). `ImageSource` (img.rs:41-50): `Resource` (from `&str`/
`String`/`SharedString`: URI if it parses as one, else embedded asset path, img.rs:62-86; from `&Path`/
`PathBuf`/`Arc<Path>`: file, img.rs:88-104), `Render(Arc<RenderImage>)`, `Image(Arc<Image>)`,
`Custom(Fn(&mut Window,&mut App) -> Option<Result<Arc<RenderImage>, ImageCacheError>>)`. Use `PathBuf`
for disk files (a `"C:/x.png"` string may parse as a URI with scheme `c` - UNVERIFIED). `StyledImage`:
`object_fit(ObjectFit::{Fill, Contain(default), Cover, ScaleDown, None})` (img.rs:139, style.rs:29-41),
`grayscale(bool)`, `with_loading(Fn()->AnyElement)` (shown after `LOADING_DELAY` 200ms, img.rs:31,
382-399), `with_fallback(Fn()->AnyElement)` on error (img.rs:371-377). Auto size = image size in image
pixels treated as logical px, aspect ratio from the image (img.rs:335-363) - at 150% DPI it renders 1.5x
physically larger; set `size_*`. Animated GIFs advance frames via `request_animation_frame` (img.rs:
314-367). Loading is async (background executor, image_cache.rs:269-282). Caching: default is the
window asset cache (`window.use_asset`, img.rs:520-526) which never evicts; free with
`ImageSource::remove_asset(cx)` (img.rs:556). Scoped cache: `image_cache(retain_all("id"))` wrapper
element or `img(..).image_cache(&entity)` (image_cache.rs:15, 330; img.rs:227);
`RetainAllImageCache::{new, clear, remove, len}` - doc says "LRU" but it retains everything until
cleared/dropped (image_cache.rs:227-317).

**anchored** `anchored()` (elements/anchored.rs:27-36): defaults `anchor(Corner::TopLeft)`,
`AnchoredFitMode::SwitchAnchor`, `AnchoredPositionMode::Window`. Options: `.anchor(Corner)`,
`.position(Point)` (absolute window point in Window mode, anchored.rs:276; offset from the anchored's
own layout origin in Local mode, anchored.rs:281-287; default = its own layout origin), `.offset(Point)`,
`.position_mode(..)`, `.snap_to_window()`, `.snap_to_window_with_margin(edges)` (anchored.rs:40-77).
Layout node is `position: Absolute, display: Flex` - takes no space (anchored.rs:111-117). SwitchAnchor
flips per axis only if the flipped box fits, and the flipped box is built from the anchor point
**without the offset** (anchored.rs:157-182) - a gap set via `.offset()` disappears when it flips.
Then it always snaps into the window, with margin + `client_inset` (anchored.rs:184-207); final offset
is rounded (anchored.rs:209-216). Anchored still paints in tree order inside ancestor clips - wrap in
`deferred()` for popups.

**deferred** `deferred(child).with_priority(n)` (also `.priority(n)`) (elements/deferred.rs:7-28, 92):
layout stays in place (deferred.rs:43-52); prepaint calls `window.defer_draw(child, element_offset,
priority)` (deferred.rs:54-66) so it paints after the whole tree, higher priority on top
(window.rs:2750-2775). The deferred draw keeps the text-style and element-id stacks but **not** the
content mask (window.rs:2163-2177), so it escapes `overflow_hidden` clipping. A `deferred` inside a
deferred panics: "cannot call defer_draw during deferred drawing" (window.rs:2184-2188).

**animation** `Animation::new(Duration)` (oneshot, linear) `.repeat()`, `.with_easing(fn)`
(elements/animation.rs:14-49). `elem.with_animation(id, animation, |elem, delta| elem...)` and
`with_animations(id, Vec<Animation>, |elem, ix, delta| ..)` (chained, run in sequence)
(animation.rs:52-88); `AnimationElement::map_element` (animation.rs:103). State (start instant, index)
is element state under `id`, created on first render (animation.rs:141-146): to replay, change the id.
Oneshot holds delta 1.0 when done (animation.rs:152-160). Easing output must stay in 0..=1
(debug_assert, animation.rs:167-170). Easing fns: `linear`, `quadratic`, `ease_in_out`,
`ease_out_quint()`, `bounce(easing)`, `pulsating_between(min, max)` (animation.rs:213-262;
`pulsating_between` with values outside 0..1 trips the assert). While running it calls
`window.request_animation_frame()` each frame (animation.rs:175-177), which notifies the **current view**
next frame (window.rs:1654-1657).

**list vs uniform_list**
- `uniform_list(id, item_count, Fn(Range<usize>, &mut Window, &mut App) -> Vec<impl IntoElement>)`
  (elements/uniform_list.rs:22-56): every row has the height of one measured row (item 0 or
  `with_width_from_item(Some(ix))`), measured with **MinContent** width (uniform_list.rs:650-672) - so
  wrapping text is measured unwrapped. Base style `overflow_y: Scroll` (uniform_list.rs:30-31).
  Default `ListSizingBehavior::Auto` needs a definite height from the parent; `Infer` sizes to
  `row_h * count` capped by available (uniform_list.rs:264-297). `with_horizontal_sizing_behavior(
  ListHorizontalSizingBehavior::Unconstrained)` for horizontal scroll (list.rs:129-135),
  `with_decoration`, `y_flipped(bool)`, `track_scroll(UniformListScrollHandle)` (uniform_list.rs:
  614-700). Handle: `new()`, `scroll_to_item(ix, ScrollStrategy::{Top,Center,Bottom})`,
  `scroll_to_item_strict`, `..._with_offset(ix, strategy, offset_items)`, `logical_scroll_top_index`,
  `is_scrollable`, `y_flipped` (uniform_list.rs:132-230); scrolls apply in the next prepaint
  (uniform_list.rs:352-357).
- `list(ListState, FnMut(usize, &mut Window, &mut App) -> AnyElement)` (elements/list.rs:24-35): variable
  heights. `ListState::new(count, ListAlignment::{Top, Bottom}, overdraw: Pixels)` (list.rs:216);
  `measure_all()` (list.rs:236), `reset(count)`, `item_count`, `splice(old_range, new_count)`,
  `splice_focusable`, `set_scroll_handler(|&ListScrollEvent{visible_range,count,is_scrolled}, ..|)`,
  `logical_scroll_top() -> ListOffset{item_ix, offset_in_item}`, `scroll_by`, `scroll_to`,
  `scroll_to_reveal_item`, `bounds_for_item`, `viewport_bounds`, scrollbar helpers (list.rs:244-475).
  Visible items are re-rendered and measured every frame as separate layout roots; off-screen heights
  are cached (list.rs:636-662) and only invalidated by `splice`/`reset` or a list width change
  (list.rs:1025-1037). Store `ListState` in the view; it has no id.
- Use `uniform_list` for long uniform rows (logs, tables); `list` when rows differ in height; plain
  `div().children(..)` lays out and paints everything.

**surface** `surface(source)` - `SurfaceSource` only has a macOS `CVPixelBuffer` variant
(elements/surface.rs:10-15); not usable on Windows.

## 5. Practical rules and gotchas

1. SVG icons need `.text_color(..)` on the `svg()` itself (svg.rs:110) and a box with the icon's aspect
   (svg_renderer.rs:87).
2. Borders are painted **after** children (style.rs:632-700, `continuation` first), so a child's
   background cannot cover the parent's border; corner radii are clamped to the box
   (style.rs:620-624). The overflow mask shrinks by border widths only when the border colour is visible
   (style.rs:568-576).
3. `opacity` is not a compositing group: each quad/glyph/shadow gets alpha multiplied
   (window.rs:2816-2826, 2847-2852, 3117) - overlapping children of a translucent div show through each
   other.
4. Truncation inherits: put `truncate()`/`line_clamp()` on the element that directly holds the text,
   not a layout container (window.rs:1440-1446).
5. Text in a flex row: make the text's container `flex_1().min_w_0()` or `overflow_hidden()` so it can
   shrink below content width (style.rs:1168-1182).
6. Text only wraps against a definite width (text.rs:347-354); min-content measurements (uniform_list
   row measurement, `list` with `Infer`) see one line.
7. `Pixels` inner field is `pub(crate)` (geometry.rs:2573): use `f32::from(px)` (geometry.rs:2788),
   `Pixels::ZERO/MAX/MIN`, `.floor()/.round()/.ceil()` (geometry.rs:2678-2705). `Rems(pub f32)`,
   `Rems::to_pixels(rem)` (geometry.rs:3112-3116), `AbsoluteLength::to_pixels(rem)` (geometry.rs:3211),
   `DefiniteLength::to_pixels(base, rem)` (geometry.rs:3358). `Bounds` helpers: `new`, `from_corners`,
   `from_corner_and_size`, `centered_at`, `intersects`, `intersect`, `union`, `dilate`, `extend`,
   `inset`, `contains`, `localize`, `center`, `is_empty`, `space_within` (geometry.rs:813-1573).
8. Spacing/sizing helpers are rem-based and follow `rem_size`; border helpers are px (see section 1).
9. `Visibility::Hidden` keeps layout and hitbox insertion in prepaint but skips paint (div.rs:1809-1811);
   `hidden()` (`Display::None`) removes the node from layout.
10. `deferred` escapes clips and paints on top; nesting it panics (window.rs:2184-2188).
11. `with_runs` panics on wrong lengths (text.rs:236-244); `shape_line` must not get `\n`
    (text_system.rs:372-375).
12. A grid only has equal `1fr` tracks (taffy.rs:308-314); for fixed + flexible columns use flex rows.

### Performance notes
- **Text shaping** is cached per window across exactly two frames: lines used this frame are kept,
  everything else is dropped at `finish_frame` (text_system/line_layout.rs:393-466). Static text is
  cheap; text that changes every frame (timers, live logs) is reshaped each frame. Each text element
  also caches its measured size inside the frame (text.rs:373-378).
- `LineWrapper` caches char widths per (font, size) and is pooled (line_wrapper.rs:10-11, 193-209;
  text_system.rs:292-306); truncation is per-char arithmetic, not reshaping.
- `uniform_list` renders the measured row twice per frame (request_layout and prepaint,
  uniform_list.rs:262, 338) plus the visible range; keep row construction light.
- `list` lays out visible rows as separate roots each frame; `measure_all()` renders every row on the
  first frame (list.rs:109-116, 560-600).
- Animations and GIFs re-render the whole current view every frame (window.rs:1654-1657); put the
  animated part in its own small view (Entity) so the rest is not rebuilt.
- SVGs are rasterised once per (path, device size) into the sprite atlas (window.rs:3086-3096);
  animating an SVG's size re-parses and re-rasterises per new size (svg_renderer.rs:83-102).
- Images decode on the background executor; global asset cache never evicts (img.rs:520-526) - large
  or many images need `remove_asset` or a `RetainAllImageCache` you clear.
- Each `BoxShadow` is one GPU `Shadow` primitive (window.rs:2817-2827); relative cost of large blur
  radii vs quads - UNVERIFIED (shader not read).
- Paths are tessellated by lyon on `build()` (path_builder.rs:244); building paths inside a canvas
  paint closure repeats that every frame - cache built `Path`s if static.
- Taffy layout runs over the whole frame tree each frame (taffy.rs:151-232); many plain children cost
  layout + paint even when off-screen - virtualise long collections with the list elements.
