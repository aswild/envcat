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

enum Pattern {
    Empty,
    Glob(GlobSet),
    Regex(RegexSet),
}

impl Pattern {
    fn is_match(&self, name: &[u8]) -> bool {
        #[cfg(unix)]
        fn globs_matche_bytes(globs: &GlobSet, name: &[u8]) -> bool {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            globs.is_match(<OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(name))
        }

        #[cfg(windows)]
        fn globs_matche_bytes(globs: &GlobSet, name: &[u8]) -> bool {
            globs.is_match(&*String::from_utf8_lossy(name))
        }

        match self {
            Self::Empty => true,
            Self::Glob(globs) => globs_matche_bytes(globs, name),
            Self::Regex(regexes) => regexes.is_match(name),
        }
    }
}

/// Iterator over NULL separated chunks of a byte slice.
///
/// Consecutive NULLs, or a slice starting with NULL, will yield empty slices from the iterator.
/// However if the input slice ends with NULL, an empty slice will not be yielded at the end of
/// iteration (for implementation reasons).
///
/// Is this materially better than `buf.split(|b| *b == 0u8)`? Probably not, but it was fun to
/// write.
struct NullSplit<'a>(&'a [u8]);

impl<'a> Iterator for NullSplit<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.0.is_empty() {
            None
        } else {
            match memchr::memchr(0u8, self.0) {
                Some(pos) => {
                    let ret = &self.0[..pos];
                    self.0 = &self.0[(pos + 1)..];
                    Some(ret)
                }
                None => {
                    let ret = self.0;
                    self.0 = &[];
                    Some(ret)
                }
            }
        }
    }
}

fn print_env<W: Write>(buf: &[u8], out: &mut W, pattern: &Pattern) -> io::Result<()> {
    for chunk in NullSplit(buf) {
        if chunk.is_empty() {
            continue;
        }

        // slice::split_once is unstable so do it ourselves
        let (key, val) = match memchr::memchr(b'=', chunk) {
            Some(pos) => {
                let (left, right) = chunk.split_at(pos);
                (left, &right[1..])
            }
            None => (chunk, [].as_slice()),
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

#[cfg(test)]
#[test]
fn test_nullsplit() {
    let mut i = NullSplit(b"abc\0def\0ghi\0jkl\0");
    assert_eq!(i.next(), Some(b"abc".as_slice()));
    assert_eq!(i.next(), Some(b"def".as_slice()));
    assert_eq!(i.next(), Some(b"ghi".as_slice()));
    assert_eq!(i.next(), Some(b"jkl".as_slice()));
    assert_eq!(i.next(), None);

    let mut i = NullSplit(b"");
    assert_eq!(i.next(), None);

    let mut i = NullSplit(b"\0one\0two");
    assert_eq!(i.next(), Some(b"".as_slice()));
    assert_eq!(i.next(), Some(b"one".as_slice()));
    assert_eq!(i.next(), Some(b"two".as_slice()));
    assert_eq!(i.next(), None);

    let mut i = NullSplit(b"one\0\0\0two");
    assert_eq!(i.next(), Some(b"one".as_slice()));
    assert_eq!(i.next(), Some(b"".as_slice()));
    assert_eq!(i.next(), Some(b"".as_slice()));
    assert_eq!(i.next(), Some(b"two".as_slice()));
    assert_eq!(i.next(), None);

    let mut i = NullSplit(b"abc");
    assert_eq!(i.next(), Some(b"abc".as_slice()));
    assert_eq!(i.next(), None);
}
