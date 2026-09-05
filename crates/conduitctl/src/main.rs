//! Binaire `conduitctl`.

#![forbid(unsafe_code)]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime tokio");
    match runtime.block_on(conduitctl::run(args)) {
        Ok(out) => print!("{out}"),
        Err(conduitctl::CliError::Usage(msg)) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("erreur : {e}");
            std::process::exit(1);
        }
    }
}
