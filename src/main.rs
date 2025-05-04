use clap::Parser;
use history_grep::open_and_parse_history_file;
use history_grep::process_patterns;
use history_grep::CaseMode;
use stderrlog::LogLevelNum;

#[derive(clap::Parser)]
/// History Grep (hgr) -- A simple tool for searching through (bash) command
/// history files.
struct Args {
    /// Increase debug level
    #[arg(short, long, action=clap::ArgAction::Count)]
    debug: u8,

    /// The history file to read. Default is $HISTFILE
    #[arg(short = 'f', long)]
    histfile: Option<String>,

    /// Use case-sensitive search. Default is non-sensitive
    #[arg(short = 's', long)]
    case_sensitive: bool,

    /// Exclude commands matching these patterns.
    #[arg(short = 'v', long, action=clap::ArgAction::Append)]
    exclude: Vec<String>,

    /// The patterns to search for.
    ///
    /// The patterns can appear in the command in any order. hgr searches
    /// for an exact match. A regular expression pattern can be specified
    /// by enclosing a term in slashes, e.g., `/foo[Bb]ar/`. Additional
    /// slashes inside the pattern are allowed.
    #[arg()]
    patterns: Vec<String>,
}

fn main() {
    if let Err(e) = actual_main() {
        log::error!("{:?}", e);
        std::process::exit(1);
    }
}

fn actual_main() -> Result<(), anyhow::Error> {
    let args = Args::parse();
    let log_level = match args.debug {
        // We don't really use Warn, so 0 is really
        // just Error
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

    for (idx, entry) in entries.iter().enumerate() {
        if entry.matches(&inc_patterns, &excl_patterns) {
            println!("{:x} {}", idx, entry);
        }
    }

    Ok(())
}
