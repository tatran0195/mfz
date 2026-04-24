fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("icon.ico");
        res.set("ProductName", "MFZ");
        res.set(
            "FileDescription",
            "AWS MFA + SSM port forwarding + RDP session manager",
        );
        res.set("LegalCopyright", "Copyright © 2026 Waka");
        res.set("InternalName", "mfz.exe");
        res.set("OriginalFilename", "mfz.exe");
        res.compile().unwrap();
    }
}
