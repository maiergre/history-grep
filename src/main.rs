fn main() {
    if let Err(e) = history_grep::actual_main() {
        log::error!("{:?}", e);
        std::process::exit(1);
    }
}
