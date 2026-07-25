fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=icons/icon.ico");

        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("icons/icon.ico");
        resource.compile().expect("failed to embed Windows icon");
    }
}
