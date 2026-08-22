use std::fmt;
use std::io::{self, BufWriter, Stdout, Write};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use crossterm::terminal::{
    BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    disable_raw_mode, enable_raw_mode,
};
use crossterm::{Command, execute};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::TerminalOptions;
use ratatui::Viewport;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Widget};

const INLINE_VIEWPORT_HEIGHT: u16 = 12;

pub(super) type AppTerminal = Terminal<CrosstermBackend<BufWriter<Stdout>>>;

#[derive(Debug, Clone, Copy)]
struct EnableAlternateScroll;

impl Command for EnableAlternateScroll {
    fn write_ansi(&self, formatter: &mut impl fmt::Write) -> fmt::Result {
        write!(formatter, "\x1b[?1007h")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other("alternate scroll requires ANSI support"))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy)]
struct DisableAlternateScroll;

impl Command for DisableAlternateScroll {
    fn write_ansi(&self, formatter: &mut impl fmt::Write) -> fmt::Result {
        write!(formatter, "\x1b[?1007l")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other("alternate scroll requires ANSI support"))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

pub(super) fn draw_synchronized(terminal: &mut AppTerminal, render: impl FnOnce(&mut Frame<'_>)) -> anyhow::Result<()> {
    execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let draw_result = terminal.draw(render).map(|_| ());
    let end_result = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    draw_result?;
    end_result?;
    Ok(())
}

pub(super) fn clear_synchronized(terminal: &mut AppTerminal) -> anyhow::Result<()> {
    execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let clear_result = terminal.autoresize();
    let end_result = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    clear_result?;
    end_result?;
    Ok(())
}

pub(super) fn reset_inline_synchronized(terminal: &mut AppTerminal) -> anyhow::Result<()> {
    execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let reset_result = (|| -> anyhow::Result<()> {
        terminal.autoresize()?;
        terminal.clear()?;
        // Keep this as one ordered sequence. Some terminals, including tmux, only purge
        // scrollback reliably when the visible screen is cleared before CSI 3 J.
        terminal
            .backend_mut()
            .write_all(b"\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H")?;
        terminal.backend_mut().flush()?;
        execute!(terminal.backend_mut(), Hide)?;
        Ok(())
    })();
    let end_result = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    reset_result?;
    end_result?;
    Ok(())
}

pub(super) fn insert_history_lines(terminal: &mut AppTerminal, lines: Vec<Line<'static>>) -> anyhow::Result<()> {
    for chunk in lines.chunks(usize::from(u16::MAX)) {
        let height = u16::try_from(chunk.len()).unwrap_or(u16::MAX);
        let text = Text::from(chunk.to_vec());
        terminal.insert_before(height, move |buffer| {
            let area = buffer.area;
            let content = Rect::new(
                area.x.saturating_add(1),
                area.y,
                area.width.saturating_sub(2),
                area.height,
            );
            Paragraph::new(text).render(content, buffer);
        })?;
    }
    Ok(())
}

pub(super) struct TerminalSession {
    inline_terminal: AppTerminal,
    picker_terminal: Option<AppTerminal>,
    keyboard_enhancement: bool,
    picker_mode: bool,
}

impl TerminalSession {
    pub(super) fn enter() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(
            stdout,
            Clear(ClearType::Purge),
            Clear(ClearType::All),
            MoveTo(0, 0),
            Hide
        ) {
            let _ = execute!(stdout, Show);
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        let keyboard_enhancement = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
        let terminal = match inline_terminal(stdout) {
            Ok(terminal) => terminal,
            Err(error) => {
                let mut stdout = io::stdout();
                if keyboard_enhancement {
                    let _ = execute!(stdout, PopKeyboardEnhancementFlags);
                }
                let _ = execute!(stdout, Show);
                let _ = disable_raw_mode();
                return Err(error.into());
            }
        };
        Ok(Self {
            inline_terminal: terminal,
            picker_terminal: None,
            keyboard_enhancement,
            picker_mode: false,
        })
    }

    pub(super) fn terminal(&mut self) -> &mut AppTerminal {
        if self.picker_mode {
            self.picker_terminal.as_mut().expect("picker terminal is active")
        } else {
            &mut self.inline_terminal
        }
    }

    pub(super) fn set_picker_mode(&mut self, picker_mode: bool) -> anyhow::Result<()> {
        if self.picker_mode == picker_mode {
            return Ok(());
        }

        if picker_mode {
            self.inline_terminal.backend_mut().flush()?;
            execute!(
                self.inline_terminal.backend_mut(),
                EnterAlternateScreen,
                EnableAlternateScroll,
                Clear(ClearType::All),
                MoveTo(0, 0),
                Hide
            )?;
            match Terminal::new(CrosstermBackend::new(BufWriter::new(io::stdout()))) {
                Ok(terminal) => self.picker_terminal = Some(terminal),
                Err(error) => {
                    let _ = execute!(
                        self.inline_terminal.backend_mut(),
                        DisableAlternateScroll,
                        LeaveAlternateScreen
                    );
                    return Err(error.into());
                }
            }
            self.picker_mode = true;
        } else {
            let picker_terminal = self.picker_terminal.as_mut().expect("picker terminal is active");
            picker_terminal.backend_mut().flush()?;
            execute!(
                picker_terminal.backend_mut(),
                DisableAlternateScroll,
                LeaveAlternateScreen,
                Hide
            )?;
            self.picker_terminal = None;
            self.picker_mode = false;
        }
        Ok(())
    }

    pub(super) fn reset_inline(&mut self) -> anyhow::Result<()> {
        debug_assert!(!self.picker_mode);
        reset_inline_synchronized(&mut self.inline_terminal)?;
        self.inline_terminal = inline_terminal(io::stdout())?;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if self.picker_mode {
            if let Some(picker_terminal) = self.picker_terminal.as_mut() {
                let _ = execute!(
                    picker_terminal.backend_mut(),
                    DisableAlternateScroll,
                    LeaveAlternateScreen
                );
            }
            self.picker_terminal = None;
            self.picker_mode = false;
        }
        if self.keyboard_enhancement {
            let _ = execute!(self.inline_terminal.backend_mut(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(self.inline_terminal.backend_mut(), Show);
        let _ = disable_raw_mode();
        let _ = self.inline_terminal.show_cursor();
        let _ = finish_terminal_output(self.inline_terminal.backend_mut());
    }
}

fn finish_terminal_output(output: &mut impl Write) -> io::Result<()> {
    output.write_all(b"\r\n")?;
    output.flush()
}

fn inline_terminal(stdout: Stdout) -> io::Result<AppTerminal> {
    let backend = CrosstermBackend::new(BufWriter::new(stdout));
    let viewport_height = crossterm::terminal::size()
        .map(|(_, height)| height.clamp(1, INLINE_VIEWPORT_HEIGHT))
        .unwrap_or(INLINE_VIEWPORT_HEIGHT);
    Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(viewport_height),
        },
    )
}

#[cfg(test)]
#[path = "terminal_test.rs"]
mod terminal_test;
