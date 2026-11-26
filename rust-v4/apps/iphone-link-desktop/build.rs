fn main() {
    println!("cargo:rerun-if-changed=ui/main_window.slint");
    println!("cargo:rerun-if-changed=ui/icons.slint");
    println!("cargo:rerun-if-changed=ui/assets/logo.png");
    slint_build::compile("ui/main_window.slint").expect("Failed to compile ui/main_window.slint");
}
