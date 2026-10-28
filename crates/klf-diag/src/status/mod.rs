//! The status surface a command draws while it works, and the print macros
//! that keep other output out of its way.
//!
//! It lives here rather than in the `klf` binary because it is not the `klf`
//! binary's: every generated driver links `klf-frontend-rt` and runs this same
//! surface, so the KLF Kira compiler, a `KArc` driver and every other definition
//! package report their phases the way `klf` does, from one implementation.
//!
//! A title line and the last few phases, redrawn in place on stderr. Only when
//! stderr is a terminal: piped output belongs to whatever is reading it, and a
//! log full of cursor-movement escapes helps nobody.
//!
//! # Why stderr, and why in place
//!
//! stdout carries the command's *result* — the artifact path, the value a
//! program returned, the diagnostics a tool parses — and progress is not part of
//! that. Drawing in place keeps the surface to a fixed height, so a two-minute
//! build scrolls nothing away.
//!
//! # Why the ticker repaints one row and not the block
//!
//! The surface is repainted every [`TICK`] so the elapsed timer stays live
//! inside a phase that reports nothing for seconds, and so the shimmer keeps
//! sweeping. Repainting the *block* at that rate is what makes a status display
//! flicker: the terminal paints an emptied rectangle and then a filled one,
//! twenty times a second, and the eye catches the gap. So the ticker rewrites
//! only the title row — one [`redraw_title`], one cell range cleared — and the
//! history rows are repainted only when the history actually changes, which is
//! on a phase and nothing else.
use std::fmt::Write as _;
use std::io::{IsTerminal, Write};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

pub mod timings;

use crate::progress::ProgressSink;

/// Prints to stdout with the status surface stood aside first.
///
/// The surface erases itself by moving the cursor up over the rows it drew.
/// Anything printed between the last redraw and that erase moves the cursor too,
/// so the erase walks up from the wrong place and wipes the *output* instead of
/// the surface. Suspending first is the fix, and going through these macros is
/// what makes it hold: a bare `println!` added later is the bug coming back.
#[macro_export]
macro_rules! out {
    ($($argument:tt)*) => {{
        let _suspended = $crate::progress::suspended();
        println!($($argument)*);
        $crate::progress::resumed();
    }};
}

/// [`out`] for stderr: diagnostics, refusals, and every `klf: …` failure.
#[macro_export]
macro_rules! err {
    ($($argument:tt)*) => {{
        let _suspended = $crate::progress::suspended();
        eprintln!($($argument)*);
        $crate::progress::resumed();
    }};
}

/// [`out`] for a payload written without a trailing newline: emitted KIR,
/// formatted source, a module dump.
#[macro_export]
macro_rules! raw {
    ($($argument:tt)*) => {{
        let _suspended = $crate::progress::suspended();
        print!($($argument)*);
        $crate::progress::resumed();
    }};
}

/// Prints a rendered diagnostic block to stderr, ending it on its own row.
///
/// Producers are not uniform: a definition's own message may bring a trailing
/// newline, while `render_kept_diagnostics` joins its lines with newlines and
/// leaves the last one off. Printing such a block verbatim drops the row it ends
/// on, and on a terminal the shell prompt then continues the last diagnostic.
/// [`block`] settles it, so every producer prints the same way.
#[macro_export]
macro_rules! raw_err {
    ($($argument:tt)*) => {{
        let _suspended = $crate::progress::suspended();
        eprint!("{}", $crate::status::block(&format!($($argument)*)));
        $crate::progress::resumed();
    }};
}

/// A rendered block with exactly one trailing newline.
#[must_use]
pub fn block(text: &str) -> String {
    if text.ends_with('\n') {
        text.to_owned()
    } else {
        format!("{text}\n")
    }
}

/// How many recent phases stay on screen.
const VISIBLE: usize = 6;

/// The widest line drawn, short enough that an 80-column terminal never wraps
/// one status row into two physical rows and breaks the redraw.
const WIDTH: usize = 72;

/// How often the surface repaints itself between phases.
///
/// A repaint is only ever cheaper than the phase it interrupts: one title row.
/// This is what keeps the elapsed timer live while a single phase (analysis,
/// lowering, a link) holds the command for seconds without reporting anything
/// in between. Fast enough that the shimmer glides instead of stepping.
const TICK: Duration = Duration::from_millis(50);

/// How many cells wide the bright band sweeping the title is: a short comet
/// rather than a wash over the whole line.
const SHIMMER_WIDTH: usize = 6;

/// A drawn status surface.
pub struct Surface {
    state: Mutex<State>,
}

/// Everything the surface redraws from.
struct State {
    title: String,
    started: Instant,
    history: Vec<String>,
    drawn: usize,
    /// Repaint counter. Advanced by the ticker as well as by phases, so the
    /// shimmer keeps sweeping even while the command says nothing new.
    frame: u64,
    /// Set while something else owns the terminal (see [`ProgressSink::suspend`]).
    /// The ticker skips repaints until the display is cleared again, so a
    /// diagnostic printed mid-command is not immediately painted over.
    suspended: bool,
    /// Set by [`Surface::finish`]. Tells the ticker to exit on its next wake.
    done: bool,
    /// Whether ANSI styling is wanted on this stderr: off for `NO_COLOR` and for
    /// a terminal that declares itself `dumb`.
    styled: bool,
}

impl Surface {
    /// Installs a surface titled `title`, when stderr is a terminal.
    ///
    /// Returns `None` when it is not, and installs nothing — a piped command
    /// stays exactly as quiet as it was.
    #[must_use]
    pub fn install(title: &str) -> Option<Arc<Self>> {
        if !std::io::stderr().is_terminal() {
            return None;
        }
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
        let dumb = std::env::var_os("TERM").is_some_and(|term| term == "dumb");
        let surface = Arc::new(Self {
            state: Mutex::new(State {
                title: title.to_owned(),
                started: Instant::now(),
                history: Vec::new(),
                drawn: 0,
                frame: 0,
                suspended: false,
                done: false,
                styled: !no_color && !dumb,
            }),
        });
        crate::progress::install(surface.clone());
        // Paint once up front, so the title, its shimmer and its timer are on
        // screen from 0.0s rather than only once the first phase arrives.
        if let Ok(mut state) = surface.state.lock() {
            Surface::draw(&mut state);
        }
        spawn_ticker(&surface);
        Some(surface)
    }

    /// Erases the surface and stops receiving phases.
    ///
    /// The surface is scratch: what a command has to say is on stdout, and
    /// leaving a half-drawn status above it would compete with that.
    pub fn finish(&self) {
        crate::progress::uninstall();
        if let Ok(mut state) = self.state.lock() {
            state.done = true;
            erase(&mut state);
        }
    }

    /// Redraws the whole surface from `state`.
    ///
    /// Styling is applied *after* [`clamp`], so the ANSI escapes never count
    /// toward the drawn width: they are zero-width on screen, and counting them
    /// would wrap a styled row into two physical rows and break the redraw math.
    fn draw(state: &mut State) {
        let mut out = std::io::stderr().lock();
        let mut buffer = String::new();
        // Back up over what was drawn last time, so the surface stays put
        // instead of scrolling.
        for _ in 0..state.drawn {
            buffer.push_str("\x1b[1A\x1b[2K");
        }
        buffer.push_str(&title_row(state));
        buffer.push('\n');
        for line in &state.history {
            buffer.push_str(&clamp(&format!("  {line}")));
            buffer.push('\n');
        }
        state.drawn = row_count(state.history.len());
        let _ = out.write_all(buffer.as_bytes());
        let _ = out.flush();
    }
}

/// How many rows the surface occupies: the title and the phase rows.
///
/// One function, because [`Surface::draw`], [`erase`] and [`repaint_title`] all
/// have to agree on it, and one of them being off by a row is invisible until a
/// terminal shows it: the erase lands one row short of the title and takes the
/// first phase line with it.
fn row_count(history: usize) -> usize {
    history + 1
}

/// The title row: the command's words under the shimmer, and its plain timer.
fn title_row(state: &State) -> String {
    let elapsed = state.started.elapsed().as_secs_f32();
    // The timer stays plain: the shimmer sweeps the words only, so the numbers
    // never flicker between shades and stay readable at a glance.
    let timer = format!(" ({elapsed:.1}s)");
    let room = WIDTH.saturating_sub(timer.chars().count());
    let name: String = state.title.chars().take(room).collect();
    if state.styled {
        format!("{}{timer}", shimmer(&name, state.frame))
    } else {
        format!("{name}{timer}")
    }
}

/// The bytes that repaint the title row alone, for the ticker.
///
/// One row cleared and rewritten, with the cursor put back where it was: the
/// phase rows under it are byte-identical to what is already on screen, and
/// repainting them is what flickers.
///
/// Every row of the surface ends in a newline, so the cursor sits one row
/// *below* the last one and the title is a whole `drawn` rows up — the same
/// distance [`erase`] covers. This is where the two must not drift: a repaint one
/// row short erases the first phase line and draws the title over it.
fn repaint_title(state: &State) -> String {
    let rows = state.drawn;
    let mut buffer = String::new();
    let _ = write!(buffer, "\x1b[{rows}A\r\x1b[2K");
    buffer.push_str(&title_row(state));
    let _ = write!(buffer, "\x1b[{rows}B\r");
    buffer
}

/// Writes [`repaint_title`] to stderr.
fn redraw_title(state: &mut State) {
    if state.drawn == 0 {
        return;
    }
    let mut out = std::io::stderr().lock();
    let _ = out.write_all(repaint_title(state).as_bytes());
    let _ = out.flush();
}

/// Repaints the surface on a timer until it is finished or dropped.
///
/// Progress reporting is event-driven — [`Surface::draw`] runs per phase — and
/// the long phases hold the command for seconds without emitting one. Without
/// this the elapsed timer visibly stalls and only jumps when the next phase
/// lands. The ticker holds only a [`Weak`] handle, so it exits on its own once
/// the last [`Surface`] is gone; progress stays best-effort and a failed spawn
/// is silently no ticker rather than a failed command.
fn spawn_ticker(surface: &Arc<Surface>) {
    let weak: Weak<Surface> = Arc::downgrade(surface);
    let _ = std::thread::Builder::new()
        .name("klf-progress".to_owned())
        .spawn(move || {
            loop {
                std::thread::sleep(TICK);
                let Some(surface) = weak.upgrade() else {
                    return;
                };
                let Ok(mut state) = surface.state.lock() else {
                    continue;
                };
                if state.done {
                    return;
                }
                if state.suspended || state.drawn == 0 {
                    continue;
                }
                state.frame = state.frame.wrapping_add(1);
                redraw_title(&mut state);
            }
        });
}

/// Sweeps a whitish band across `line`, whose head sits at `frame`.
///
/// The head travels left to right and on past the end, so the tail fades out
/// off the edge instead of being cut there; then it wraps with a single dark
/// frame between passes rather than a long pause. Each cell behind the head
/// steps one shade down the 256-colour grey ramp (white at the head, fading cell
/// by cell into the plain text), which is what makes the falloff a gradient
/// rather than a hard edge. `line` must already be width-limited — escapes are
/// zero-width, so styling first and limiting second would over-count and wrap.
fn shimmer(line: &str, frame: u64) -> String {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    let span = u64::try_from(chars.len() + SHIMMER_WIDTH).unwrap_or(u64::MAX);
    let head = usize::try_from(frame % span).unwrap_or(0);
    let mut out = String::new();
    for (index, cell) in chars.iter().enumerate() {
        let behind = head.saturating_sub(index);
        let Some(code) = (index <= head && behind < SHIMMER_WIDTH).then(|| {
            // One grey step per cell back from the head: 255 at the front,
            // fading down the ramp behind it.
            let shade = 255u8.saturating_sub(u8::try_from(behind).unwrap_or(0));
            if behind == 0 {
                format!("1;38;5;{shade}")
            } else {
                format!("38;5;{shade}")
            }
        }) else {
            out.push(*cell);
            continue;
        };
        out.push_str("\x1b[");
        out.push_str(&code);
        out.push('m');
        out.push(*cell);
        out.push_str("\x1b[0m");
    }
    out
}

impl ProgressSink for Surface {
    fn suspend(&self) {
        if let Ok(mut state) = self.state.lock() {
            // Flagged before erasing under the same lock, so the ticker cannot
            // slip a repaint in between and paint over the output that asked
            // for the terminal.
            state.suspended = true;
            erase(&mut state);
        }
    }

    fn resume(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.suspended = false;
            Surface::draw(&mut state);
        }
    }

    fn phase(&self, phase: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        // A new phase means the command is talking again, which ends whatever
        // suspension stood the surface aside.
        state.suspended = false;
        state.history.push(phase.to_owned());
        if state.history.len() > VISIBLE {
            state.history.remove(0);
        }
        Surface::draw(&mut state);
    }
}

/// Erases every drawn row.
fn erase(state: &mut State) {
    if state.drawn == 0 {
        return;
    }
    let mut out = std::io::stderr().lock();
    let mut buffer = String::new();
    for _ in 0..state.drawn {
        buffer.push_str("\x1b[1A\x1b[2K");
    }
    state.drawn = 0;
    let _ = out.write_all(buffer.as_bytes());
    let _ = out.flush();
}

/// Truncates `line` to the drawn width, on a character boundary.
fn clamp(line: &str) -> String {
    if line.chars().count() <= WIDTH {
        return line.to_owned();
    }
    // One character short of the width leaves room for the ellipsis, and
    // counting characters rather than bytes keeps a multi-byte name whole.
    let kept: String = line.chars().take(WIDTH - 1).collect();
    format!("{kept}…")
}

/// Installs the display `title` is drawn on: animated by default, the
/// millisecond log under `--timings`.
///
/// The returned guard erases the display and, for a timings run, prints what is
/// left of it, however the command returns.
#[must_use]
pub fn begin(title: &str, timings: bool) -> Finish {
    if timings {
        return Finish::Timings(Some(crate::status::timings::install(title)));
    }
    Finish::Surface(Surface::install(title))
}

/// A command that draws nothing: `klf build` writing KIR to stdout, where the
/// stream belongs to the module alone.
///
/// The same type as every other command's [`Finish`], so a result line does not
/// have to ask whether there was a display to take down.
#[must_use]
pub fn silent() -> Finish {
    Finish::Surface(None)
}

/// Takes a command's display down when the command returns.
///
/// A command has many exits — every early `return` on a bad option or a failed
/// analysis — and a display left installed would draw over whatever came next.
/// Dropping this is that safety net; a command calls [`Finish::end`] first so
/// the result line lands after the phases rather than between them.
pub enum Finish {
    Surface(Option<Arc<Surface>>),
    Timings(Option<crate::status::timings::Timings>),
}

impl Finish {
    /// Takes the display down now, so what the command says about its result
    /// comes after every phase it took.
    pub fn end(&mut self) {
        match self {
            Self::Surface(surface) => {
                if let Some(surface) = surface.take() {
                    surface.finish();
                }
            }
            Self::Timings(timings) => {
                if let Some(timings) = timings.take() {
                    timings.end();
                }
            }
        }
    }
}

impl Drop for Finish {
    fn drop(&mut self) {
        self.end();
    }
}

/// Strips the `ESC[…m` sequences [`shimmer`] emits, leaving visible text.
#[cfg(test)]
fn visible(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("\x1b[") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        match tail.find('m') {
            Some(end) => rest = &tail[end + 1..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            title: "Checking definitions/mini".to_owned(),
            started: Instant::now(),
            history: Vec::new(),
            drawn: 0,
            frame: 0,
            suspended: false,
            done: false,
            styled: true,
        }
    }

    #[test]
    fn the_ticker_reaches_the_title_and_not_the_row_under_it() {
        // Every row `draw` emits ends in a newline, so the cursor is one row
        // below the last one. The repaint has to climb the whole surface to
        // reach the title: climbing one row short erases the first phase line
        // and paints the title over it, which reads as a duplicated title and a
        // phase line with half of another spliced into it.
        let mut state = state();
        state.history = vec!["units".to_owned(), "parse".to_owned()];
        state.drawn = row_count(state.history.len());
        let repaint = repaint_title(&state);
        assert!(
            repaint.starts_with(&format!("\x1b[{}A", state.drawn)),
            "climbs {} rows, wants {}: {repaint:?}",
            state.drawn,
            state.drawn
        );
        assert!(
            repaint.contains(&format!("\x1b[{}B", state.drawn)),
            "comes back down the same distance: {repaint:?}"
        );
    }

    #[test]
    fn a_repaint_leaves_exactly_one_row_cleared() {
        // The point of repainting the title alone: one `\x1b[2K`, however many
        // rows the surface has. More than one is the flicker.
        let mut state = state();
        state.history = (0..VISIBLE).map(|index| format!("phase {index}")).collect();
        state.drawn = row_count(state.history.len());
        let repaint = repaint_title(&state);
        assert_eq!(repaint.matches("\x1b[2K").count(), 1, "{repaint:?}");
    }

    #[test]
    fn an_undrawn_surface_is_never_repainted() {
        // Nothing to move over: with no rows drawn the cursor is wherever the
        // last output left it, and climbing would erase somebody's output.
        let mut state = state();
        state.drawn = 0;
        redraw_title(&mut state);
        assert_eq!(state.drawn, 0);
    }

    #[test]
    fn a_rendered_block_ends_on_its_own_row() {
        // What a definition's rejection message looks like: one line, no
        // trailing newline. Printed verbatim it runs into whatever prints next,
        // which on a terminal is the shell prompt.
        assert_eq!(
            block("./main.kira:95:1: error[2098]: no"),
            "./main.kira:95:1: error[2098]: no\n"
        );
    }

    #[test]
    fn a_block_that_already_ends_on_a_row_is_not_given_a_blank_one() {
        // What `render_kept_diagnostics` and klf-cli's own renderer produce:
        // every line newline-terminated, including the last.
        let rendered = "first\nsecond\n";
        assert_eq!(block(rendered), rendered);
    }

    #[test]
    fn a_line_within_the_width_is_left_alone() {
        assert_eq!(clamp("parsing"), "parsing");
    }

    #[test]
    fn a_long_line_is_truncated_on_a_character_boundary() {
        let long = "é".repeat(WIDTH * 2);
        let clamped = clamp(&long);
        assert_eq!(clamped.chars().count(), WIDTH);
        assert!(clamped.ends_with('…'));
        // The point of counting characters: a byte-wise cut would split one of
        // these in half and produce something that is not text.
        assert!(std::str::from_utf8(clamped.as_bytes()).is_ok());
    }

    #[test]
    fn nothing_is_installed_when_stderr_is_not_a_terminal() {
        // Under a test harness stderr is captured, never a terminal, so this
        // also pins that a piped command stays silent.
        if !std::io::stderr().is_terminal() {
            let _finish = begin("Checking", false);
            assert!(!crate::progress::listening());
        }
    }

    #[test]
    fn the_timer_is_never_styled_so_the_numbers_do_not_flicker_between_shades() {
        let state = state();
        let row = title_row(&state);
        let timer = row.rsplit_once('(').expect("a timer").1;
        assert!(
            !timer.contains('\x1b'),
            "the timer carries escapes: {timer:?}"
        );
        // What the reader sees is the title and the timer, with the shimmer
        // adding only styling over the words.
        assert_eq!(
            visible(&row),
            format!("{} ({})", state.title, timer.trim_end_matches(')'))
        );
    }

    #[test]
    fn the_title_row_never_exceeds_the_drawn_width_however_long_the_title_is() {
        let mut state = state();
        state.title = "x".repeat(WIDTH * 2);
        // Counted in visible characters: the escapes are zero-width, and
        // counting them would wrap the row and break the redraw.
        assert_eq!(visible(&title_row(&state)).chars().count(), WIDTH);
    }

    #[test]
    fn the_shimmer_keeps_its_visible_text_and_moves_with_the_frame() {
        let line = "Checking definitions/mini";
        let first = shimmer(line, 0);
        assert_eq!(visible(&first), line);
        // The band travels: a frame with the head off the end (tail fading out)
        // styles different cells than the opening frame.
        let later = shimmer(line, line.chars().count() as u64);
        assert_eq!(visible(&later), line);
        assert_ne!(first, later);
    }

    #[test]
    fn the_shimmer_is_boldest_at_the_head_and_steps_down_behind_it() {
        // Frame 2 puts the head on the third cell: that cell is bold white, the
        // two behind it are one and two grey steps down, and the fourth is plain.
        let styled = shimmer("abcdef", 2);
        assert!(styled.contains("\x1b[1;38;5;255mc"), "{styled:?}");
        assert!(styled.contains("\x1b[38;5;254mb"), "{styled:?}");
        assert!(styled.contains("\x1b[38;5;253ma"), "{styled:?}");
        assert!(styled.contains('d'), "plain text is left alone: {styled:?}");
    }

    #[test]
    fn the_shimmer_on_empty_text_is_empty() {
        assert_eq!(shimmer("", 42), "");
    }
}
