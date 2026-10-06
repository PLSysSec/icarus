use std::env;
use std::fs::File;
use std::io::prelude::*;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Error};
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use walkdir::WalkDir;

fn main() -> Result<(), Error> {
    lalrpop::Configuration::new()
        .emit_rerun_directives(true)
        .process_current_dir()
        .expect("lalrpop error");

    let mut test_cases = TestCases::default();
    test_cases.scan_dir(PathBuf::from("tests"), "samples")?;
    test_cases.scan_dir(PathBuf::from_iter(["..", "..", "vendor"]), "boogie")?;
    let test_src = test_cases.generate_src();

    let test_file_path =
        Path::new(&env::var("OUT_DIR").context("Couldn't read `OUT_DIR` environment variable")?)
            .join("test.rs");
    let mut test_file = File::create(&test_file_path).context("Couldn't create `test.rs`")?;
    write!(test_file, "{}", test_src).context("Couldn't write `test.rs`")?;

    Ok(())
}

#[derive(Default)]
struct TestCases {
    fn_idents: Vec<Ident>,
    names: Vec<String>,
    paths: Vec<String>,
}

impl TestCases {
    fn scan_dir(&mut self, base_dir_path: PathBuf, scan_dir_name: &str) -> Result<(), Error> {
        let scan_dir_path = base_dir_path.join(scan_dir_name);
        println!("cargo:rerun-if-changed={}", scan_dir_path.display());

        for entry in WalkDir::new(&scan_dir_path) {
            let entry = entry
                .with_context(|| format!("Error walking directory {}", scan_dir_path.display()))?;

            if !entry.file_type().is_file() {
                continue;
            }

            let path = entry.path();

            let extension = match path.extension() {
                Some(extension) => extension,
                None => continue,
            };
            if extension != "bpl" {
                continue;
            }

            let stripped_path = path
                .strip_prefix(&base_dir_path)
                .context("Bad path prefix")?
                .with_extension("");

            let mut name = String::new();
            for component in stripped_path.components() {
                if let Component::Normal(component) = component {
                    if !name.is_empty() {
                        name.push_str("__");
                    }
                    name.extend(
                        component
                            .to_string_lossy()
                            .chars()
                            .map(|c| if c.is_alphanumeric() { c } else { '_' }),
                    );
                }
            }

            self.fn_idents.push(format_ident!("test__{}", name));
            self.names.push(name);

            // TODO(spinda): Is there a better way to do this than
            // `to_string_lossy`?
            self.paths.push(path.to_string_lossy().into_owned());
        }

        Ok(())
    }

    fn generate_src(&self) -> TokenStream {
        let TestCases {
            fn_idents,
            names,
            paths,
        } = self;
        quote! {
            #(
                #[test]
                #[allow(non_snake_case)]
                fn #fn_idents() -> Result<(), Error> {
                    test_program(#names, #paths)
                }
            )*
        }
    }
}
