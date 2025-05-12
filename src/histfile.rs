use std::fmt::Display;
use std::fs::File;
use std::io::BufRead as _;
use std::io::BufReader;

use anyhow::Context;
use chrono::DateTime;
use chrono::Local;
use chrono::Utc;
use regex::Regex;

use crate::default_ts;
use crate::MIN_REASONABLE_UNIXTIME;

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
///   command.
/// * Once we have read at least one timestamp, we expect that each history
///   entry begins with a timestamp line and is followed by one or more
///   command lines. I.e., in this state we support multi-line commands.
pub fn parse_history_file(read: impl std::io::Read) -> anyhow::Result<Vec<HistEntry>> {
    let mut ret = Vec::new();
    let mut state = FileParseState::NoTimestamps;
    let mut cur_ts = default_ts();
    let mut cur_lines = vec![];

    let reader = BufReader::new(read);
    for (mut line_no, line) in reader.lines().enumerate() {
        line_no += 1;
        let line = line.with_context(|| format!("Error reading line number {}", line_no))?;
        let parsed = ParsedLine::parse(&line);
        log::trace!("Parsed line {}: `{}`", line_no, line);
        log::trace!("State: {:?}, parsed: {:?}", state, parsed);
        state = match (&state, parsed) {
            (FileParseState::NoTimestamps, ParsedLine::Command(cmd)) => {
                // No timestamp yet. Assume each line in the file is a single command
                ret.push(HistEntry {
                    ts: default_ts(),
                    command: cmd,
                });
                FileParseState::NoTimestamps
            }
            (FileParseState::NoTimestamps, ParsedLine::Timestamp(ts)) => {
                // Got our first timestamp
                cur_ts = ts;
                FileParseState::LastWasTimestamp
            }
            (FileParseState::LastWasTimestamp, ParsedLine::Command(cmd))
            | (FileParseState::LastWasCommand, ParsedLine::Command(cmd)) => {
                cur_lines.push(cmd);
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
                ret.push(HistEntry {
                    ts: cur_ts,
                    command: cur_lines.join("\n"),
                });
                cur_ts = ts;
                cur_lines.clear();
                FileParseState::LastWasTimestamp
            }
        }
    }
    if state == FileParseState::LastWasCommand {
        // Need to flush the last command
        ret.push(HistEntry {
            ts: cur_ts,
            command: cur_lines.join("\n"),
        });
    }

    Ok(ret)
}

/// Deduplicate consecutive history entries that have the same command.
/// The first instance of the command is retained.
pub fn dedup_entries(entries: Vec<HistEntry>) -> Vec<HistEntry> {
    let mut ret: Vec<HistEntry> = Vec::with_capacity(entries.len());
    if entries.is_empty() {
        return entries;
    }
    for e in entries.into_iter() {
        if ret.last().is_some_and(|prev| prev.command == e.command) {
            continue;
        }
        ret.push(e);
    }
    ret
}

/// Represents a history entry
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HistEntry {
    pub ts: DateTime<Utc>,
    pub command: String,
}

impl HistEntry {
    /// Check if this entry matches the search criteria.
    ///
    /// In order to be considered a match, this entry must match *all* regexes
    /// from `include_re` and it must not match *any* regex from `exclude_re`
    pub fn matches(&self, include_re: &[Regex], exclude_re: &[Regex]) -> bool {
        include_re.iter().all(|re| re.is_match(&self.command))
            && !exclude_re.iter().any(|re| re.is_match(&self.command))
    }

    pub fn ts_as_string(&self) -> String {
        let local_time = DateTime::<Local>::from(self.ts);
        let formatted_time = local_time.format("%Y-%m-%d %H:%M:%S");
        formatted_time.to_string()
    }
}

impl Display for HistEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}   {}", self.ts_as_string(), self.command)
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
enum ParsedLine {
    Timestamp(DateTime<Utc>),
    Command(String),
}

impl ParsedLine {
    /// Parse a single line. We assume that a line represents a timestamps if it
    /// has the format `#123456` and the timestamp is larger or equal to
    /// MIN_REASONABLE_UNIXTIME
    pub fn parse(line: &str) -> Self {
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

#[cfg(test)]
mod test {
    use chrono::Duration;

    use crate::default_ts;

    use super::*;

    pub fn newentry(ts: DateTime<Utc>, command: &str) -> HistEntry {
        HistEntry {
            ts,
            command: command.to_owned(),
        }
    }

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
        assert_eq!(
            ParsedLine::parse("  "),
            ParsedLine::Command("  ".to_string())
        );
        assert_eq!(ParsedLine::parse(""), ParsedLine::Command(String::new()));
    }

    #[test]
    fn test_parse_file_no_timestamp() {
        let mkentry = |cmd: &str| HistEntry {
            ts: default_ts(),
            command: cmd.to_owned(),
        };
        let expected = vec![mkentry("foo"), mkentry("bar"), mkentry("foobar baz")];
        let hist = "foo\nbar\nfoobar baz\n".as_bytes();
        assert_eq!(parse_history_file(hist).unwrap(), expected);

        // no trailing newline
        let hist = "foo\nbar\nfoobar baz".as_bytes();
        assert_eq!(parse_history_file(hist).unwrap(), expected);
    }

    #[test]
    fn test_parse_file_timestamps() {
        let mkentry = |ts, cmd: &str| HistEntry {
            ts: DateTime::from_timestamp(ts, 0).unwrap(),
            command: cmd.to_owned(),
        };
        let mkmultiline = |ts, cmds: &[&str]| HistEntry {
            ts: DateTime::from_timestamp(ts, 0).unwrap(),
            command: cmds.join("\n"),
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
            command: cmd.to_owned(),
        };
        let mkmultiline = |ts, cmds: &[&str]| HistEntry {
            ts: DateTime::from_timestamp(ts, 0).unwrap(),
            command: cmds.join("\n"),
        };
        let hist = "#1262305001\nfoobar \n#1262305005\n\nbar bar bar\n\n".as_bytes();
        let res = parse_history_file(hist).unwrap();
        assert_eq!(
            res,
            vec![
                mkentry(1262305001, "foobar "),
                mkmultiline(1262305005, &["", "bar bar bar", ""]),
            ]
        );
    }

    #[test]
    fn test_matches() {
        let mk_re = |p: &str| Regex::new(p).unwrap();
        let entry = HistEntry {
            ts: default_ts(),
            command: "I am the command\nwith many lines. Foobar".to_owned(),
        };
        assert!(entry.matches(&[mk_re("am the"), mk_re("many")], &[]));
        assert!(entry.matches(&[], &[]));
        assert!(!entry.matches(&[], &[mk_re("many")]));
        assert!(!entry.matches(&[], &[mk_re("many"), mk_re("XXXX")]));
        assert!(!entry.matches(
            &[mk_re("am the"), mk_re("many")],
            &[mk_re("many"), mk_re("XXXX")]
        ));
        assert!(!entry.matches(&[mk_re("am the"), mk_re("XXX")], &[]));
        assert!(entry.matches(&[mk_re("am the"), mk_re("am the")], &[]));
    }

    #[test]
    fn test_dedup_entries() {
        let t0 = default_ts();
        let t1 = default_ts() + Duration::minutes(5);
        let t2 = default_ts() + Duration::minutes(10);
        let t3 = default_ts() + Duration::minutes(12);
        let t4 = default_ts() + Duration::minutes(23);
        let orig = vec![
            newentry(t0, "ls -la"),
            newentry(t1, "rm foobar"),
            newentry(t2, "rm foobar"),
            newentry(t3, "rm foobar"),
            newentry(t4, "ls -la"),
        ];
        assert_eq!(
            dedup_entries(orig),
            vec![
                newentry(t0, "ls -la"),
                newentry(t1, "rm foobar"),
                newentry(t4, "ls -la"),
            ]
        );
    }
}
