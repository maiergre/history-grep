use clap::Parser as _;
use history_grep::Args;

fn main() {
    let args = Args::parse();
    if let Err(e) = history_grep::actual_main(args) {
        log::error!("{:?}", e);
        std::process::exit(1);
    }
}
