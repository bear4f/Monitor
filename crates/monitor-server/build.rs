use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=../../web/dist");

    for required in [
        "../../web/dist/index.html",
        "../../web/dist/assets",
        "../../web/dist/.csp-theme-hash",
    ] {
        if !Path::new(required).exists() {
            panic!(
                "missing {required}; build the frontend first with `cd web && npm ci && npm run build`"
            );
        }
    }
}
