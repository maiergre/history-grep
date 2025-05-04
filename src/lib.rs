use std::fmt::Display;
use std::fs::File;
use std::io::BufRead as _;
use std::io::BufReader;

use anyhow::Context;
use chrono::DateTime;
use chrono::Local;
use chrono::Utc;
use regex::Regex;
use regex::RegexBuilder;

/// Assume any "timestamps" we parse before that date are not actually
/// valid.
/// 2010-01-01 00:00:00 UTC
const MIN_REASONABLE_UNIXTIME: i64 = 1262304000;

fn default_ts() -> DateTime<Utc> {
    DateTime::from_timestamp(MIN_REASONABLE_UNIXTIME, 0).unwrap()
}

pub fn open_and_parse_history_file(histfile: &str) -> anyhow::Result<Vec<HistEntry>> {
    log::debug!("Reading and parsing history file: {}", &histfile);
    let read =
        File::open(histfile).with_context(|| format!("Opening history file: `{}`", histfile))?;
    parse_history_file(read)
}

/// Parse a history file
///
/// The parsing logic supports bash like timestamps. I.e., lines starting with
/// a `#` followed by only digits. The general logic as as folows:
/// * If we haven't read any timestamps yet, assume every line is a separate
///    command.
/// * Once we have read at least one timestamp, we expect that each history
///   entry begins with a timestamp line and is followed by one or more
///   command lines. I.e., in this state we support multi-line commands.
pub fn parse_history_file(read: impl std::io::Read) -> anyhow::Result<Vec<HistEntry>> {
    let mut ret = Vec::new();
    let mut state = FileParseState::NoTimestamps;
    let mut cur_entry = HistEntry::with_ts(default_ts());

    let reader = BufReader::new(read);
    for (mut line_no, line) in reader.lines().enumerate() {
        line_no += 1;
        let line = line.with_context(|| format!("Error reading line number {}", line_no))?;
        let parsed = ParsedLine::parse(&line);
        log::trace!("Parsed line {}: `{}`", line_no, line);
        log::trace!("State: {:?}, parsed: {:?}", state, parsed);
        state = match (&state, parsed) {
            (_, ParsedLine::Empty) => {
                log::info!(
                    "Read an empty line. Should not happen. At line: {}",
                    line_no
                );
                state
            }
            (FileParseState::NoTimestamps, ParsedLine::Command(cmd)) => {
                // No timestamp yet. Assume each line in the file is a single command
                ret.push(HistEntry {
                    ts: default_ts(),
                    lines: vec![cmd],
                });
                FileParseState::NoTimestamps
            }
            (FileParseState::NoTimestamps, ParsedLine::Timestamp(ts)) => {
                // Got our first timestamp
                cur_entry.ts = ts;
                FileParseState::LastWasTimestamp
            }
            (FileParseState::LastWasTimestamp, ParsedLine::Command(cmd))
            | (FileParseState::LastWasCommand, ParsedLine::Command(cmd)) => {
                cur_entry.lines.push(cmd);
                FileParseState::LastWasCommand
            }
            (FileParseState::LastWasTimestamp, ParsedLine::Timestamp(_ts)) => {
                log::info!(
                    "Read two consecutive lines with timestamps. At line {}: `{}`",
                    line_no,
                    line
                );
                // Ignore the timestamp. The most likely explanation is that somebody
                // had a command like `#123434`
                FileParseState::LastWasTimestamp
            }
            (FileParseState::LastWasCommand, ParsedLine::Timestamp(ts)) => {
                ret.push(cur_entry);
                cur_entry = HistEntry::with_ts(ts);
                FileParseState::LastWasTimestamp
            }
        }
    }
    if state == FileParseState::LastWasCommand {
        // Need to flush the last command
        ret.push(cur_entry);
    }

    Ok(ret)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HistEntry {
    pub ts: DateTime<Utc>,
    pub lines: Vec<String>,
}

impl HistEntry {
    pub fn with_ts(ts: DateTime<Utc>) -> Self {
        HistEntry {
            ts,
            lines: Vec::new(),
        }
    }

    pub fn matches(&self, include_re: &[Regex], exclude_re: &[Regex]) -> bool {
        let command: &str = if self.lines.len() == 1 {
            &self.lines[0]
        } else {
            &self.lines.join("\n")
        };
        include_re.iter().all(|re| re.is_match(command))
            && !exclude_re.iter().any(|re| re.is_match(command))
    }
}

impl Display for HistEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let local_time = DateTime::<Local>::from(self.ts);
        let formatted_time = local_time.format("%Y-%m-%d %H:%M:%S");
        write!(f, "{}   {}", formatted_time, self.lines.join("\n"))
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
enum FileParseState {
    /// We have not read any timestamps yet. Assume eaach line is a single
    /// command. The other two states
    NoTimestamps,
    /// The previous line was a timestamp.
    LastWasTimestamp,
    /// The previous line was a command
    LastWasCommand,
}

/// Represents a single parsed line from a history file
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedLine {
    Timestamp(DateTime<Utc>),
    Command(String),
    Empty,
}

impl ParsedLine {
    /// Parse a single line. We assume that a line represents a timestamps if it
    /// has the format `#123456` and the timestamp is larger or equal to
    /// MIN_REASONABLE_UNIXTIME
    pub fn parse(line: &str) -> Self {
        if line.is_empty() || line.chars().all(|c| c.is_whitespace()) {
            return ParsedLine::Empty;
        }
        if let Some(stripped) = line.strip_prefix('#') {
            let maybe_ts = match stripped.parse::<i64>() {
                Ok(unixtime) if unixtime >= MIN_REASONABLE_UNIXTIME => {
                    DateTime::from_timestamp(unixtime, 0)
                }
                _ => None,
            };
            if let Some(ts) = maybe_ts {
                return ParsedLine::Timestamp(ts);
            }
        }
        ParsedLine::Command(line.to_string())
    }
}

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

pub fn string_to_regex(s: &str, case_mode: CaseMode) -> Result<Regex, regex::Error> {
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

pub fn process_patterns(patterns: Vec<String>, case_mode: CaseMode) -> anyhow::Result<Vec<Regex>> {
    patterns
        .iter()
        .map(|s| {
            log::debug!("Include pattern `{}` as `{:?}`", s, case_mode);
            string_to_regex(s, case_mode).with_context(|| format!("Error parsing pattern `{}`", s))
        })
        .collect()
}

#[cfg(test)]
mod test {
    use itertools::Itertools;

    use super::*;

    #[test]
    fn test_parse_line() {
        assert_eq!(
            // Not an integer ==> parsed as command
            ParsedLine::parse("# foo bar"),
            ParsedLine::Command("# foo bar".to_owned())
        );
        // Timestamp before earliest time
        assert_eq!(
            ParsedLine::parse("#1234"),
            ParsedLine::Command("#1234".to_owned())
        );
        // Timestamp before earliest ==> parsed as command
        assert_eq!(
            ParsedLine::parse("#1262303999"),
            ParsedLine::Command("#1262303999".to_owned())
        );

        // Timestamp at or after the "earliest" date
        assert_eq!(
            ParsedLine::parse("#1262304000"),
            ParsedLine::Timestamp(DateTime::from_timestamp(1262304000, 0).unwrap())
        );
        // Trailing and leading whitespace is not accepted
        assert_eq!(
            ParsedLine::parse("#1262304422 "),
            ParsedLine::Command("#1262304422 ".to_owned())
        );
        assert_eq!(
            ParsedLine::parse(" #1262304422"),
            ParsedLine::Command(" #1262304422".to_owned())
        );

        // Not just a timestamp on the line
        assert_eq!(
            ParsedLine::parse("#1262304000 asdf foobar"),
            ParsedLine::Command("#1262304000 asdf foobar".to_owned())
        );

        // Leading and trailing whitespace is retained
        assert_eq!(
            ParsedLine::parse(" foo bar baz "),
            ParsedLine::Command(" foo bar baz ".to_owned())
        );

        // Empty lines or lines with just whitespace
        assert_eq!(ParsedLine::parse("  "), ParsedLine::Empty);
        assert_eq!(ParsedLine::parse(""), ParsedLine::Empty);
    }

    #[test]
    fn test_parse_file_no_timestamp() {
        let mkentry = |cmd: &str| HistEntry {
            ts: default_ts(),
            lines: vec![cmd.to_owned()],
        };
        let expected = vec![mkentry("foo"), mkentry("bar"), mkentry("foobar baz")];
        let hist = "foo\nbar\nfoobar baz\n".as_bytes();
        assert_eq!(parse_history_file(hist).unwrap(), expected);

        // no trailing newline
        let hist = "foo\nbar\nfoobar baz".as_bytes();
        assert_eq!(parse_history_file(hist).unwrap(), expected);

        // no empty lines
        let hist = "foo\n\nbar\nfoobar baz\n\n\n".as_bytes();
        assert_eq!(parse_history_file(hist).unwrap(), expected);
    }

    #[test]
    fn test_parse_file_timestamps() {
        let mkentry = |ts, cmd: &str| HistEntry {
            ts: DateTime::from_timestamp(ts, 0).unwrap(),
            lines: vec![cmd.to_owned()],
        };
        let mkmultiline = |ts, cmds: &[&str]| HistEntry {
            ts: DateTime::from_timestamp(ts, 0).unwrap(),
            lines: cmds.iter().map(|s| s.to_string()).collect_vec(),
        };
        // First commands have not timestamps, then we use timestamps
        let hist = "foo\n\
            foobar\n\
            #1262305001\n\
            this is a command\n\
            #1262305003\n\
            multi line\n\
            command\n\
            foo\n"
            .as_bytes();
        let res = parse_history_file(hist).unwrap();
        assert_eq!(
            res,
            vec![
                mkentry(MIN_REASONABLE_UNIXTIME, "foo"),
                mkentry(MIN_REASONABLE_UNIXTIME, "foobar"),
                mkentry(1262305001, "this is a command"),
                mkmultiline(1262305003, &["multi line", "command", "foo"]),
            ]
        );

        // Test lines that are almost a timestamp
        let hist = "#1262305001\n\
          #12623030\n\
            foo\n\
            #1262305003\n\
            bar" // now trailing newline here to mix things up a bit
        .as_bytes();
        let res = parse_history_file(hist).unwrap();
        assert_eq!(
            res,
            vec![
                mkmultiline(1262305001, &["#12623030", "foo"]),
                mkentry(1262305003, "bar"),
            ]
        );

        // Test multiple consecutive timestamps
        let hist = "#1262305001\n\
                #1262305003\n\
                foo\n\
                #1262305007\n\
                bar"
        .as_bytes();
        let res = parse_history_file(hist).unwrap();
        assert_eq!(
            res,
            vec![
                // The second timestamp is currently ignores. I guess we could also
                // interpret it as the start of a command....
                mkentry(1262305001, "foo"),
                mkentry(1262305007, "bar"),
            ]
        );
    }

    #[test]
    fn test_parse_file_timestamps_and_whitespace() {
        let mkentry = |ts, cmd: &str| HistEntry {
            ts: DateTime::from_timestamp(ts, 0).unwrap(),
            lines: vec![cmd.to_owned()],
        };
        let hist = "#1262305001\nfoobar \n#1262305005\n\nbar bar bar\n\n".as_bytes();
        let res = parse_history_file(hist).unwrap();
        assert_eq!(
            res,
            vec![
                mkentry(1262305001, "foobar "),
                mkentry(1262305005, "bar bar bar"),
            ]
        );
    }

    #[test]
    fn test_string_to_regex() {
        // Exact match
        let re = string_to_regex("/fo.o", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("/fo.o"));
        assert!(!re.is_match("/Fo.o"));
        assert!(!re.is_match("/foxo"));
        assert!(!re.is_match("/fooo"));
        assert!(re.is_match("X/fo.oX"));

        // regex
        let re = string_to_regex("/fo.o/", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("fo.o"));
        assert!(!re.is_match("Fo.o"));
        assert!(re.is_match("foxo"));
        assert!(re.is_match("fooo"));
        assert!(re.is_match("Xfo.oX"));
        assert!(re.is_match("XfoxoX"));
        assert!(re.is_match("XfoooX"));

        // Regex with interior slash
        let re = string_to_regex("/abc/f[o]+.ar/", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("abc/foobar"));
        assert!(re.is_match("abc/foXar"));
        assert!(!re.is_match("abc_foobar"));

        //  Exact match
        let re = string_to_regex("asd[12]", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("asd[12]"));
        assert!(!re.is_match("aSd[12]"));
        assert!(!re.is_match("asd1"));

        //  Exact match
        let re = string_to_regex("/asd[12]/", CaseMode::Sensitive).unwrap();
        assert!(!re.is_match("asd[12]"));
        assert!(re.is_match("asd1"));
        assert!(re.is_match("asd2"));
        assert!(re.is_match("_asd2_"));

        // anchor
        let re = string_to_regex("/^asd/", CaseMode::Sensitive).unwrap();
        assert!(re.is_match("asd__"));
        assert!(!re.is_match("_asd__"));

        // case senitivity
        let re = string_to_regex("asDf", CaseMode::Sensitive).unwrap();
        assert!(!re.is_match("XasdfX"));
        assert!(re.is_match("XasDfX"));
        let re = string_to_regex("asdf", CaseMode::Insensitive).unwrap();
        assert!(re.is_match("aSDf"));
        assert!(re.is_match("asdf"));
        let re = string_to_regex("/asDf/", CaseMode::Sensitive).unwrap();
        assert!(!re.is_match("XasdfX"));
        assert!(re.is_match("XasDfX"));
        let re = string_to_regex("/asdf/", CaseMode::Insensitive).unwrap();
        assert!(re.is_match("aSDf"));
        assert!(re.is_match("asdf"));
    }
}
