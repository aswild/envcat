use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anstyle::{AnsiColor, Style};
use clap::Parser;

const STYLE_KEY: Style = color_style(AnsiColor::Green);
const STYLE_EQU: Style = color_style(AnsiColor::Blue);
const STYLE_VAL: Style = Style::new();

const fn color_style(color: AnsiColor) -> Style {
    Style::new().fg_color(Some(anstyle::Color::Ansi(color)))
}

fn print_env<W: Write>(buf: &[u8], out: &mut W) -> io::Result<()> {
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
    /// File path, omit or specify '-' to read stdin
    file: Option<PathBuf>,
}

fn run() -> anyhow::Result<()> {
    let args = Args::parse();
    let data = match &args.file {
        None => read_stdin()?,
        Some(path) if path == Path::new("-") => read_stdin()?,
        Some(path) => std::fs::read(path)?,
    };

    let mut out = anstream::stdout().lock();
    print_env(&data, &mut out)?;

    Ok(())
}

fn read_stdin() -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    io::stdin().lock().read_to_end(&mut buf)?;
    Ok(buf)
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
