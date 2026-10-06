// path: crates/polygloss-app/src/fixture.rs
//
// Every form ADR-0031's rules name, in an app file. Each `// accept` or
// `// reject N` line starts a case; spacing-tokens.test.ts scans each case
// on its own and expects no finding, or one of rule N.

// accept: a zero length
div().w(px(0.));
// accept: a zero length, written out
div().h(px(0.0));
// accept: rem helpers at 0, full, auto and fractions
div().p_0().w_full().min_h_0().min_w_0().size_full().h_auto().w_1_2();
// accept: the 1 pt border, on every side
div().border_1().border_t_1().border_b_1().rounded_full().rounded_none();
// accept: colors are not dimensions
div().text_color(theme.foreground).bg(theme.background.opacity(0.5));
// accept: a token
div().px(px(space::edge::CANVAS)).gap(px(space::gap::CONTROLS));
// accept: centring arithmetic
div().left(px(w / 2.0)).top(px((h - space::size::ICON) / 2.));
// accept: 0, 1, 2 and 0.5 bindings
let zero = 0.0;
let one = 1.;
let two = 2.0;
let half = 0.5;
// accept: geometry arithmetic outside the viewport is a review item
let reach = 0.75 * h;
// accept: a const of a non-geometry type
const RETRIES: u32 = 3;
const STEP: Duration = Duration::from_millis(16);
// accept: a derived const of a geometry type
const EDGE: f32 = space::edge::CANVAS;
// accept: a struct field set from a token
let style = CardStyle { margin_x: space::edge::CANVAS, gap: space::gap::CARDS, ..Default::default() };
// accept: a geometry field set to none
let anchor = ScrollAnchor { file: 0, offset_px: 0.0 };
// accept: a non-geometry struct field
let opts = Options { columns: 160.0, ratio: 0.3 };
// accept: a small Button
let b = Button::new("x").small();
// accept: an extra-small Button at the XS radius
let b = Button::new("x").xsmall().rounded(px(radius::XS));
// accept: a multi-line chain that sets its size
let b = Button::new("x")
    .ghost()
    .small()
    .on_click(cx.listener(|this, _, _, cx| this.go(cx)));
// accept: a numbered tuple field is not a literal
let x = pair.0 + pair.1;
// accept: string contents are not code
let label = "px(12.) .p_2() const PAD: f32 = 6.0;";

// reject 1: a literal in px
div().w(px(12.));
// reject 1: one point is still a literal
div().h(px(1.));
// reject 1: a negative literal
div().mt(px(-1.));
// reject 1: "unbounded" as a literal
div().max_w(px(100_000.));
// reject 1: Pixels::from
let w: Pixels = Pixels::from(12.);
// reject 1: a literal into Pixels
let w: Pixels = 12.0.into();
// reject 1: rems
div().min_h(rems(1.2));
// reject 1: a scaled literal inside px
div().h(px((size * 1.54).round()));
// reject 1: relative
div().h(relative(1.2));
// reject 2: rem padding
div().px_3();
// reject 2: a rem half-step gap
div().gap_1p5();
// reject 2: a rem size
div().size_4();
// reject 2: a rem inset
div().top_1();
// reject 2: a negative rem margin
div().mt_neg_1();
// reject 2: a 1 px rem helper
div().w_px();
// reject 2: a rem radius
div().rounded_md();
// reject 2: a 2 pt border
div().border_2();
// reject 3: text_xs
div().text_xs();
// reject 3: text_base
div().text_base();
// reject 3: text_xl
div().text_xl();
// reject 3: text_2xl
div().text_2xl();
// reject 3: a literal line height
div().line_height(px(18.));
// reject 3: a relative line height
div().line_height(relative(1.2));
// reject 4: a numeric const
const PAD: f32 = 6.0;
// reject 4: a tuple const
const WINDOW_SIZE: (f32, f32) = (1200.0, 800.0);
// reject 4: a Pixels static
static RAIL: Pixels = px(3.);
// reject 4: a let binding of a literal
let w = 13.0;
// reject 4: a typed let binding of a literal
let gap: f32 = 6.;
// reject 5: a margin field
let style = CardStyle { margin_x: 16.0, ..Default::default() };
// reject 5: a height field
let metrics = Metrics { row_height: 20.0, ..Default::default() };
// reject 5: a gap field
let style = Style { card_gap: 12.0 };
// reject 5: a border field
let style = Style { border: 1.0 };
// reject 7: a multi-line chain with no size
let b = Button::new("x")
    .ghost()
    .label("Go")
    .on_click(|_, _, _| {});
// reject 7: an extra-small Button at the kit radius
let b = Button::new("x").xsmall().ghost();
