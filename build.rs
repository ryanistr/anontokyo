fn main() {
    if std::env::var_os("CARGO_FEATURE_GUI").is_some() {
        compile_ui();
    }
}

// maps the "@material" import in ui/*.slint to the vendored component set
#[cfg(feature = "gui")]
fn compile_ui() {
    let config = slint_build::CompilerConfiguration::new().with_library_paths(
        std::collections::HashMap::from([(
            "material".to_string(),
            std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
                .join("ui/material-1.1.0/material.slint"),
        )]),
    );
    slint_build::compile_with_config("ui/anontokyo.slint", config).unwrap();
}

#[cfg(not(feature = "gui"))]
fn compile_ui() {}
