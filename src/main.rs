use std::io::{self, Read, Write};

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

/// `GlobSet` only matches on `AsRef<Path>` types, extend it to accept arbitrary bytes as input too,
/// since we're using it to match key names rather than file paths.
trait GlobExt {
    fn is_match_bytes(&self, needle: &[u8]) -> bool;
}

impl GlobExt for GlobSet {
    fn is_match_bytes(&self, needle: &[u8]) -> bool {
        // On unix, Path and OsStr are interchangeable with `&[u8]`
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            self.is_match(OsStr::from_bytes(needle))
        }

        // Otherwise (e.g. on windows) we can't directly interchange bytes and OsStr, so loop through
        // UTF-8 to get a string that implements AsRef<Path>
        #[cfg(not(unix))]
        {
            let s = String::from_utf8_lossy(needle);
            self.is_match(&*s)
        }
    }
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
            Self::Glob(globs) => globs.is_match_bytes(name),
            Self::Regex(regexes) => regexes.is_match(name),
        }
    }
}

/// pretty-print a key/value pair to `out`
fn write_pair<W: Write>(out: &mut W, key: &[u8], val: &[u8]) -> io::Result<()> {
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

    /// Sort the list by <name>
    #[arg(short = 'S', long)]
    sort: bool,

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

    let buf = match &path {
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

    let mut data: Vec<(&[u8], &[u8])> = buf
        .split(|b| *b == b'\0')
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| {
            // [T].split_once() is still unstable :(
            match chunk.iter().position(|b| *b == b'=') {
                Some(pos) => {
                    let (left, right) = chunk.split_at(pos);
                    (left, &right[1..])
                }
                None => (chunk, [].as_slice()),
            }
        })
        .collect();

    if args.sort {
        data.sort_by_key(|(key, _val)| *key);
    }

    let mut out = anstream::stdout().lock();
    for (key, val) in data.into_iter() {
        if pattern.is_match(key) {
            write_pair(&mut out, key, val)?;
        }
    }

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
