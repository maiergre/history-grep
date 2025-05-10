use std::num::ParseIntError;

use anyhow::Context;
use chrono::DateTime;
use chrono::Utc;
use clap::Parser;
use crossterm::clipboard::CopyToClipboard;
use crossterm::execute;
use crossterm::tty::IsTty as _;
use histfile::open_and_parse_history_file;
use regex::Regex;
use regex::RegexBuilder;
use stderrlog::LogLevelNum;

mod histfile;

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

#[derive(clap::Parser)]
/// History Grep (hgr) -- A simple tool for searching through (bash) command
/// history files.
///
/// The output is similar to bash's `history`: `<ID> <DATE> <COMMAND>`
/// WARNING: The ID is different from the one bash uses/produces and as such
/// it must not be used with `!` history expansion.
pub struct Args {
    /// Increase debug level
    #[arg(short, long, action=clap::ArgAction::Count)]
    debug: u8,

    /// The history file to read. Default is $HISTFILE
    #[arg(short = 'f', long)]
    histfile: Option<String>,

    /// Gets the history entry with `ID` from the history file, prints it, and
    /// copies it to the clipboard.
    #[arg(long, visible_alias = "cp", value_name = "ID", value_parser = parse_hex_to_usize)]
    copy: Option<usize>,

    /// Use case-sensitive search. Default is non-sensitive
    #[arg(short = 's', long, conflicts_with = "copy")]
    case_sensitive: bool,

    /// Exclude commands matching these patterns.
    #[arg(short = 'v', long, action=clap::ArgAction::Append, conflicts_with = "copy")]
    exclude: Vec<String>,

    /// The patterns to search for.
    ///
    /// The patterns can appear in the command in any order. hgr searches
    /// for an exact match. A regular expression pattern can be specified
    /// by enclosing a term in slashes, e.g., `/foo[Bb]ar/`. Additional
    /// slashes inside the pattern are allowed.
    #[arg(conflicts_with = "copy")]
    patterns: Vec<String>,
}

pub fn actual_main() -> Result<(), anyhow::Error> {
    let args = Args::parse();
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

    let case_mode = CaseMode::from_sensitive(args.case_sensitive);
    let inc_patterns = process_patterns(args.patterns, case_mode)?;
    let excl_patterns = process_patterns(args.exclude, case_mode)?;

    let histfile = match args.histfile {
        Some(histfile) => histfile,
        None => match std::env::var("HISTFILE") {
            Ok(histfile) => histfile,
            Err(_) => {
                return Err(anyhow::Error::msg(
                    "No histfile argument given and no `HISTFILE` environment variable",
                ))
            }
        },
    };

    let entries = open_and_parse_history_file(&histfile)?;
    log::debug!("Read {} history entries", entries.len());

    if let Some(idx) = args.copy {
        if idx >= entries.len() {
            return Err(anyhow::anyhow!(
                "No entry {:x} in history. Maximum entry id is {:x}",
                idx,
                entries.len()
            ));
        }
        let cmd = entries[idx].lines.join("\n");
        println!("{}", cmd);
        if std::io::stdout().is_tty() {
            execute!(std::io::stdout(), CopyToClipboard::to_clipboard_from(cmd))?;
            println!("Copied to clipboard");
        } else {
            log::warn!("Cannot copy to clipboard. Not a TTY");
        }
        return Ok(());
    }

    for (idx, entry) in entries.iter().enumerate() {
        if entry.matches(&inc_patterns, &excl_patterns) {
            println!("{:x} {}", idx, entry);
        }
    }

    Ok(())
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
pub fn pattern_to_regex(s: &str, case_mode: CaseMode) -> Result<Regex, regex::Error> {
    let pattern = if let Some(pattern) = s.strip_prefix("/").and_then(|s| s.strip_suffix("/")) {
        log::debug!("Pattern `{}` is a regex pattern", s);
        pattern.to_owned()
    } else {
        log::debug!("Pattern `{}` is fixed", s);
        regex::escape(s)
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(case_mode == CaseMode::Insensitive)
        .unicode(true)
        .build()
}

/// Convert a Vec of patterns to a Vec of regexes
pub fn process_patterns(patterns: Vec<String>, case_mode: CaseMode) -> anyhow::Result<Vec<Regex>> {
    patterns
        .iter()
        .map(|s| {
            log::debug!("Include pattern `{}` as `{:?}`", s, case_mode);
            pattern_to_regex(s, case_mode).with_context(|| format!("Error parsing pattern `{}`", s))
        })
        .collect()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_string_to_regex() {
        // Exact match
        let re = pattern_to_regex("/fo.o", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("/fo.o"));
        assert!(!re.is_match("/Fo.o"));
        assert!(!re.is_match("/foxo"));
        assert!(!re.is_match("/fooo"));
        assert!(re.is_match("X/fo.oX"));

        // regex
        let re = pattern_to_regex("/fo.o/", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("fo.o"));
        assert!(!re.is_match("Fo.o"));
        assert!(re.is_match("foxo"));
        assert!(re.is_match("fooo"));
        assert!(re.is_match("Xfo.oX"));
        assert!(re.is_match("XfoxoX"));
        assert!(re.is_match("XfoooX"));

        // Regex with interior slash
        let re = pattern_to_regex("/abc/f[o]+.ar/", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("abc/foobar"));
        assert!(re.is_match("abc/foXar"));
        assert!(!re.is_match("abc_foobar"));

        //  Exact match
        let re = pattern_to_regex("asd[12]", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("asd[12]"));
        assert!(!re.is_match("aSd[12]"));
        assert!(!re.is_match("asd1"));

        //  Exact match
        let re = pattern_to_regex("/asd[12]/", CaseMode::Sensitive).unwrap();
        assert!(!re.is_match("asd[12]"));
        assert!(re.is_match("asd1"));
        assert!(re.is_match("asd2"));
        assert!(re.is_match("_asd2_"));

        // anchor
        let re = pattern_to_regex("/^asd/", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("asd__"));
        assert!(!re.is_match("_asd__"));

        // case senitivity
        let re = pattern_to_regex("asDf", CaseMode::Sensitive).unwrap();
        assert!(!re.is_match("XasdfX"));
        assert!(re.is_match("XasDfX"));
        let re = pattern_to_regex("asdf", CaseMode::Insensitive).unwrap();
        assert!(re.is_match("aSDf"));
        assert!(re.is_match("asdf"));
        let re = pattern_to_regex("/asDf/", CaseMode::Sensitive).unwrap();
        assert!(!re.is_match("XasdfX"));
        assert!(re.is_match("XasDfX"));
        let re = pattern_to_regex("/asdf/", CaseMode::Insensitive).unwrap();
        assert!(re.is_match("aSDf"));
        assert!(re.is_match("asdf"));
    }
}
