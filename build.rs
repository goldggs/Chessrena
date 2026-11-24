fn main() {
    println!("cargo:rerun-if-changed=assets/chessrena.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("assets/chessrena.ico");
        resource.set("ProductName", "Chessrena");
        resource.set("FileDescription", "Chessrena agent chess match viewer");
        resource.set("CompanyName", "Chessrena");
        resource
            .compile()
            .expect("could not embed the Chessrena Windows application icon");
    }
}
