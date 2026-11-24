use std::{env, fs, path::PathBuf};
fn main() {
    println!("cargo:rerun-if-changed=examples");
    let mut names: Vec<_> = fs::read_dir("examples")
        .unwrap()
        .map(Result::unwrap)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .map(|p| p.file_stem().unwrap().to_str().unwrap().to_owned())
        .collect();
    names.sort();
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("lab_examples.rs"),
        format!("&{names:?}"),
    )
    .unwrap();
}
