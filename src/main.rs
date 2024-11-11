use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;

use anstyle::{AnsiColor, Style};
use anyhow::Context as _;
use clap::Parser;
use globset::{GlobBuilder, GlobSet};
use regex::bytes::{RegexSet, RegexSetBuilder};

const STYLE_KEY: Style = color_style(AnsiColor::Green);
const STYLE_EQU: Style = color_style(AnsiColor::Blue);
const STYLE_VAL: Style = Style::new();

const fn color_style(color: AnsiColor) -> Style {
    Style::new().fg_color(Some(anstyle::Color::Ansi(color)))
}

enum Pattern {
    Empty,
    Glob(GlobSet),
    Regex(RegexSet),
}

impl Pattern {
    fn is_match(&self, name: &[u8]) -> bool {
        match self {
            Self::Empty => true,
            Self::Glob(globs) => globs.is_match(OsStr::from_bytes(name)),
            Self::Regex(regexes) => regexes.is_match(name),
        }
    }
}

fn print_env<W: Write>(buf: &[u8], out: &mut W, pattern: &Pattern) -> io::Result<()> {
    for chunk in buf.split(|b| *b == b'\0') {
        if chunk.is_empty() {
            continue;
        }

        // slice::split_once is unstable so do it ourselves
        let (key, val) = match chunk.iter().position(|b| *b == b'=') {
            Some(pos) => {
                let (left, right) = chunk.split_at(pos);
                (left, &right[1..])
            }
            // constructing an empty slice is annoying, because &[] is considered a reference to
            // a zero-length array, so then we have to index it to actually get a slice.
            None => (chunk, &[][..]),
        };

        if !pattern.is_match(key) {
            continue;
        }

        // [u8] isn't Display so do it ourselves
        STYLE_KEY.write_to(out)?;
        out.write_all(key)?;
        STYLE_KEY.write_reset_to(out)?;
        write!(out, "{}={}", STYLE_EQU.render(), STYLE_EQU.render_reset())?;
        if !val.is_empty() {
            STYLE_VAL.write_to(out)?;
            out.write_all(val)?;
            STYLE_VAL.write_reset_to(out)?;
        }
        out.write_all(b"\n")?;
    }

    Ok(())
}

/// Pretty-print files of the format `<name>=<value>\0`
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// FILE is a process' PID instead of a file path.
    ///
    /// This is a shorthand for reading `/proc/<pid>/environ`
    #[arg(short, long, requires = "file")]
    pid: bool,

    /// PATTERN is a glob instead of regex
    #[arg(short, long, requires = "pattern")]
    glob: bool,

    /// PATTERN is case-sensitive
    #[arg(short = 's', long, requires = "pattern")]
    case_sensitive: bool,

    /// File path, omit or specify '-' to read stdin.
    ///
    /// When using --pid, this is a process ID number
    file: Option<String>,

    /// Filter the variable names based on patterns
    ///
    /// Include variables matching any pattern in this list. By default, patterns are
    /// case-insensitive regexes, but this can be modified using the -g/--glob and
    /// -s/--case-sensitive flags.
    pattern: Option<Vec<String>>,
}

fn run() -> anyhow::Result<()> {
    let args = Args::parse();
    let path = if args.pid {
        let pid = args
            .file
            .expect("pid option but no file")
            .parse::<u32>()
            .context("failed to parse PID argument as integer")?;
        Some(format!("/proc/{pid}/environ"))
    } else {
        match args.file.as_deref() {
            Some("-") | None => None,
            Some(path) => Some(path.into()),
        }
    };

    let pattern = match (args.pattern, args.glob) {
        (None, _) => Pattern::Empty,
        (Some(ref pats), true) => {
            let mut builder = GlobSet::builder();
            for pat in pats {
                builder.add(
                    GlobBuilder::new(pat)
                        .case_insensitive(!args.case_sensitive)
                        .build()
                        .with_context(|| format!("invalid glob '{pat}'"))?,
                );
            }
            Pattern::Glob(builder.build().context("failed to build GlobSet")?)
        }
        (Some(ref pats), false) => {
            let mut builder = RegexSetBuilder::new(pats);
            builder.case_insensitive(!args.case_sensitive);
            Pattern::Regex(builder.build().context("failed to build RegexSet")?)
        }
    };

    let data = match &path {
        Some(path) => std::fs::read(path).with_context(|| format!("failed to read {path}"))?,
        None => {
            let mut buf = Vec::new();
            io::stdin()
                .lock()
                .read_to_end(&mut buf)
                .context("failed to read stdin")?;
            buf
        }
    };

    let mut out = anstream::stdout().lock();
    print_env(&data, &mut out, &pattern)?;

    Ok(())
}

fn main() {
    if let Err(err) = run() {
        match err.downcast_ref::<io::Error>() {
            Some(io_err) if io_err.kind() == io::ErrorKind::BrokenPipe => (),
            _ => eprintln!("Error: {err:#}"),
        }
        std::process::exit(1);
    }
}
