use std::env;
use std::fs;

use clap::CommandFactory;
use clap_complete::{Shell, generate_to};
use clap_mangen::Man;

include!("src/cli.rs");

fn main() {
    println!("cargo:rerun-if-changed=src/cli.rs");
    let out_dir = env::var("OUT_DIR").unwrap();
    let mut cmd = Cli::command();

    for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
        generate_to(shell, &mut cmd, "mirage", &out_dir).unwrap();
    }

    let man = Man::new(cmd);
    let mut buf = Vec::new();
    man.render(&mut buf).unwrap();
    fs::write(std::path::Path::new(&out_dir).join("mirage.1"), buf).unwrap();
}
