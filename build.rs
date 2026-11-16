// Intègre l'icône et les informations de version dans l'exécutable Windows.
fn main() {
    println!("cargo:rerun-if-changed=assets/ferroterm.ico");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/ferroterm.ico")
            .set("ProductName", "Ferroterm")
            .set("FileDescription", "Ferroterm — terminal SSH, fichiers distants et shells locaux")
            .set("LegalCopyright", "");
        if let Err(e) = res.compile() {
            println!("cargo:warning=ressources Windows non intégrées : {e}");
        }
    }
}
