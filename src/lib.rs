use std::io::Write;
use std::num::ParseIntError;

use anyhow::Context;
use anyhow::anyhow;
use base64::prelude::BASE64_STANDARD;
use chrono::DateTime;
use chrono::Utc;
use histfile::dedup_entries;
use histfile::open_and_parse_history_file;
use interactive::run_interactive;
use itertools::Itertools as _;
use ratatui::crossterm::tty::IsTty as _;
use regex::Regex;
use regex::RegexBuilder;
use stderrlog::LogLevelNum;

mod histfile;
mod interactive;

/// Assume any "timestamps" we parse before that date are not actually
/// valid.
/// 2010-01-01 00:00:00 UTC
const MIN_REASONABLE_UNIXTIME: i64 = 1262304000;

fn default_ts() -> DateTime<Utc> {
    DateTime::from_timestamp(MIN_REASONABLE_UNIXTIME, 0).unwrap()
}

fn parse_hex_to_usize(s: &str) -> Result<usize, ParseIntError> {
    usize::from_str_radix(s, 16)
}

/// History Grep (hgr) -- A simple tool for searching through (bash) command
/// history files.
///
/// The output is similar to bash's `history`: `<ID> <DATE> <COMMAND>`
/// WARNING: The ID is different from the one bash uses/produces and as such
/// it must not be used with `!` history expansion.
#[derive(clap::Parser)]
#[command(version)]
pub struct Args {
    /// Increase debug level
    #[arg(short, long, action=clap::ArgAction::Count)]
    debug: u8,

    /// The history file to read. Default is $HISTFILE
    #[arg(short = 'f', long)]
    histfile: Option<String>,

    /// If set, do *not* de-duplicate repeated commands
    #[arg(long)]
    no_dedup: bool,

    /// Gets the history entry with `ID` from the history file, prints it, and
    /// copies it to the clipboard.
    #[arg(long, visible_alias = "cp", value_name = "ID", value_parser = parse_hex_to_usize)]
    copy: Option<usize>,

    /// Run interactive mode.
    ///
    /// Interactive mode allows interactive filtering and selection of a history entry.
    /// Type search terms separated by spaces (see `patterns`), however, no regexes are
    /// supported at this time. The list of history entries will be interactively filtered.
    /// Use arrow keys and PgUp/PgDown to navigate. Esc to quit, Enter to select an entry.
    ///
    /// [PATTERNS] are used as the initial search terms (again, no regex support though)
    ///
    /// The selected entry will be printed and copied to the clipboard
    #[arg(short = 'i', long, conflicts_with = "copy")]
    interactive: bool,

    /// For integration with bash's `bind -x` readline support.
    ///
    /// hgr is started in interactive mode and the search term(s) are seeded
    /// with the contents of the `READLINE_LINE` env variable. If an entry is selected
    /// its written to the file `TMPFILE`. The selected entry is *not* written to
    /// stdout nor copied to the clipboard.
    #[arg(long, conflicts_with = "copy", value_name = "TMPFILE")]
    bash_readline_mode: Option<String>,

    /// Use case-sensitive search. Default is non-sensitive
    #[arg(short = 's', long, conflicts_with = "copy")]
    case_sensitive: bool,

    /// Exclude commands matching these patterns.
    #[arg(short = 'v', long, action=clap::ArgAction::Append, conflicts_with = "copy")]
    exclude: Vec<String>,

    /// Only display the last N *matching* entries (default is to show only as many entries
    /// the height of the current terminal on TTYs)
    #[arg(short = 'n', long, value_name = "N", conflicts_with_all = ["copy", "interactive", "show_all"])]
    tail: Option<usize>,

    /// Show all entries (default is to show only as many entries the height of the
    /// current terminal on TTYs)
    #[arg(short = 'a', long, conflicts_with_all = ["copy", "interactive", "tail"])]
    show_all: bool,

    /// The patterns to search for.
    ///
    /// The patterns can appear in the command in any order. hgr searches
    /// for an exact match. A regular expression pattern can be specified
    /// by enclosing a term in slashes, e.g., `/foo[Bb]ar/`. Additional
    /// slashes inside the pattern are allowed.
    #[arg(conflicts_with = "copy")]
    patterns: Vec<String>,
}

pub fn actual_main(args: Args) -> Result<(), anyhow::Error> {
    let log_level = match args.debug {
        0 => LogLevelNum::Warn,
        1 => LogLevelNum::Info,
        2 => LogLevelNum::Debug,
        _ => LogLevelNum::Trace,
    };
    stderrlog::new()
        .verbosity(log_level)
        .init()
        .expect("Failed to setup logging");

    let histfile = match args.histfile {
        Some(histfile) => histfile,
        None => match std::env::var("HISTFILE") {
            Ok(histfile) => histfile,
            Err(_) => {
                return Err(anyhow::Error::msg(
                    "No histfile argument given and no `HISTFILE` environment variable",
                ));
            }
        },
    };

    let entries = open_and_parse_history_file(&histfile)?;
    let entries = if args.no_dedup {
        log::debug!("Read {} history entries", entries.len());
        entries
    } else {
        let orig_len = entries.len();
        let deduped = dedup_entries(entries);
        log::debug!(
            "Read {} history entries, {} entries after dedup",
            orig_len,
            deduped.len()
        );
        deduped
    };

    if let Some(idx) = args.copy {
        if idx >= entries.len() {
            return Err(anyhow::anyhow!(
                "No entry {:x} in history. Maximum entry id is {:x}",
                idx,
                entries.len()
            ));
        }
        let cmd = &entries[idx].command;
        if std::io::stdout().is_tty() {
            std::io::stdout().write_all(&copy_to_clipboard_seq(cmd))?;
            println!("Copied to clipboard");
        } else {
            log::warn!("Cannot copy to clipboard. Not a TTY");
        }
        return Ok(());
    }

    let case_mode = CaseMode::from_sensitive(args.case_sensitive);
    let excl_patterns = process_magic_patterns(args.exclude, case_mode)?;

    if let Some(output) = args.bash_readline_mode {
        log::debug!("Using bash_readline_mode output file `{}`", output);
        let mut fp = std::fs::File::options()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&output)
            .with_context(|| format!("Opening bash-readline-mode output file `{}`", &output))?;
        let initial_search = std::env::var("READLINE_LINE").unwrap_or_default();

        let selected = run_interactive(entries, initial_search.clone(), excl_patterns, case_mode)?;

        if let Some(selected) = selected {
            log::debug!("Selected command is `{}`", selected.command);
            fp.write_all(selected.command.as_bytes())?;
        }
    } else if args.interactive {
        if !std::io::stdout().is_tty() {
            return Err(anyhow!("stdout is not a TTY. Cannot use interactive mode"));
        }
        let initial_search = args.patterns.join(" ");
        let selected = run_interactive(entries, initial_search, excl_patterns, case_mode)?;
        if let Some(selected) = selected {
            println!("{}", selected.command);
            std::io::stdout().write_all(&copy_to_clipboard_seq(&selected.command))?;
            println!("Copied to clipboard");
        }
    } else {
        let inc_patterns = process_magic_patterns(args.patterns, case_mode)?;
        let iter = entries
            .iter()
            .enumerate()
            .filter(|(_idx, entry)| entry.matches(&inc_patterns, &excl_patterns));
        let tail = if std::io::stdout().is_tty() && !args.show_all {
            // we are on a TTY and `--show-all` wasn't used ==> only show as many entries
            // as fit the height of the terminal
            ratatui::crossterm::terminal::size()
                .ok()
                .map(|(_cols, rows)| rows as usize)
        } else {
            // show all entries
            None
        }
        // unless `--tail` is explicitly specified
        .or(args.tail);
        if let Some(tail) = tail {
            for (idx, entry) in iter.tail(tail) {
                println!("{:x} {}", idx, entry);
            }
        } else {
            for (idx, entry) in iter {
                println!("{:x} {}", idx, entry);
            }
        }
    }
    Ok(())
}

pub fn copy_to_clipboard_seq(s: &str) -> Vec<u8> {
    use base64::Engine as _;

    // Use the OSC52 ANSI sequence to copy the history entry to the
    // clipboard:
    // `\x1b]52`: 0x1b is ESC, followed by `]52`, followed by `;`
    // Followe by the clipboard to copy to (`c` is the only one that's widely
    // supported), followed by another `;` followed by
    // the base64 encoded data to be copied and finally a `BEL` (0x7)
    let mut osc52_copy_seq = "\x1b]52;c;".as_bytes().to_vec();
    // base64 encoded string
    let encoded = BASE64_STANDARD.encode(s);
    osc52_copy_seq.extend_from_slice(encoded.as_bytes());
    osc52_copy_seq.push(0x7);
    osc52_copy_seq
}

/// If searches are case-sensitive or case-insensitive
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaseMode {
    Sensitive,
    Insensitive,
}

impl CaseMode {
    pub fn from_sensitive(is_sensitive: bool) -> Self {
        if is_sensitive {
            CaseMode::Sensitive
        } else {
            CaseMode::Insensitive
        }
    }
}

/// Convert a pattern into a regex. See [`Args::pattern`]. If the given pattern
/// is enclosed in slashes, e.g., `/foo[Bb]ar/` it's assumed to be a regex.
/// Otherwise its interpreted as a "fixed" pattern.
///
pub fn magic_pattern_to_regex(magic_pat: &str, case_mode: CaseMode) -> Result<Regex, regex::Error> {
    let pattern = if let Some(pattern) = magic_pat
        .strip_prefix("/")
        .and_then(|s| s.strip_suffix("/"))
    {
        log::debug!("Pattern `{}` is a regex pattern", magic_pat);
        pattern.to_owned()
    } else {
        log::debug!("Pattern `{}` is fixed", magic_pat);
        regex::escape(magic_pat)
    };
    raw_pattern_to_regex(&pattern, case_mode)
}

/// Convert a raw pattern into a regex. No processing is performed on the patterns,
/// we just use it as-is.
pub fn raw_pattern_to_regex(pattern: &str, case_mode: CaseMode) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pattern)
        .case_insensitive(case_mode == CaseMode::Insensitive)
        .unicode(true)
        .build()
}

/// Convert a Vec of magic patterns to a Vec of regexes
pub fn process_magic_patterns(
    magic_patterns: Vec<String>,
    case_mode: CaseMode,
) -> anyhow::Result<Vec<Regex>> {
    magic_patterns
        .iter()
        .map(|s| {
            log::debug!("Include pattern `{}` as `{:?}`", s, case_mode);
            magic_pattern_to_regex(s, case_mode)
                .with_context(|| format!("Error parsing pattern `{}`", s))
        })
        .collect()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_string_to_regex() {
        // Exact match
        let re = magic_pattern_to_regex("/fo.o", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("/fo.o"));
        assert!(!re.is_match("/Fo.o"));
        assert!(!re.is_match("/foxo"));
        assert!(!re.is_match("/fooo"));
        assert!(re.is_match("X/fo.oX"));

        // regex
        let re = magic_pattern_to_regex("/fo.o/", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("fo.o"));
        assert!(!re.is_match("Fo.o"));
        assert!(re.is_match("foxo"));
        assert!(re.is_match("fooo"));
        assert!(re.is_match("Xfo.oX"));
        assert!(re.is_match("XfoxoX"));
        assert!(re.is_match("XfoooX"));

        // Regex with interior slash
        let re = magic_pattern_to_regex("/abc/f[o]+.ar/", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("abc/foobar"));
        assert!(re.is_match("abc/foXar"));
        assert!(!re.is_match("abc_foobar"));

        //  Exact match
        let re = magic_pattern_to_regex("asd[12]", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("asd[12]"));
        assert!(!re.is_match("aSd[12]"));
        assert!(!re.is_match("asd1"));

        //  Exact match
        let re = magic_pattern_to_regex("/asd[12]/", CaseMode::Sensitive).unwrap();
        assert!(!re.is_match("asd[12]"));
        assert!(re.is_match("asd1"));
        assert!(re.is_match("asd2"));
        assert!(re.is_match("_asd2_"));

        // anchor
        let re = magic_pattern_to_regex("/^asd/", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("asd__"));
        assert!(!re.is_match("_asd__"));

        // case senitivity
        let re = magic_pattern_to_regex("asDf", CaseMode::Sensitive).unwrap();
        assert!(!re.is_match("XasdfX"));
        assert!(re.is_match("XasDfX"));
        let re = magic_pattern_to_regex("asdf", CaseMode::Insensitive).unwrap();
        assert!(re.is_match("aSDf"));
        assert!(re.is_match("asdf"));
        let re = magic_pattern_to_regex("/asDf/", CaseMode::Sensitive).unwrap();
        assert!(!re.is_match("XasdfX"));
        assert!(re.is_match("XasDfX"));
        let re = magic_pattern_to_regex("/asdf/", CaseMode::Insensitive).unwrap();
        assert!(re.is_match("aSDf"));
        assert!(re.is_match("asdf"));
    }
}
