fn main() {
    sedge_gen::Build::new("templates")
        .library("forage_ui", "../forage-ui/templates")
        .run();
}
