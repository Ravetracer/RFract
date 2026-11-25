//! Lists the classes of the class libraries below the given folders that do not parse.
fn main() {
    let dirs: Vec<std::path::PathBuf> = std::env::args().skip(1).map(Into::into).collect();
    let r = rfr_core::formula::library::ClassRegistry::from_folders(&dirs);
    for (n, e) in &r.errors {
        println!("{n}\t{e}");
    }
}
