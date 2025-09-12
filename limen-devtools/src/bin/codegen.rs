//! limen-codegen: render a minimal typed graph module from a config file.
use std::fs;
use std::path::PathBuf;
use limen_devtools::{render_simple_chain_module, SimpleChainConfig};

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("usage: limen-codegen <config.json|toml> <out.rs>");
    let out = args.next().expect("usage: limen-codegen <config.json|toml> <out.rs>");
    let content = fs::read_to_string(&input).expect("read config");
    let cfg: SimpleChainConfig = if input.ends_with(".json") {
        serde_json::from_str(&content).expect("parse json")
    } else {
        toml::from_str(&content).expect("parse toml")
    };
    let code = render_simple_chain_module(&cfg);
    fs::write(&out, code).expect("write out");
    println!("Generated: {}", out);
}
