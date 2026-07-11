use std::path::PathBuf;

use cua_driver_testkit::e2e::{
    read_json_lines, validate_catalog, CaseDeclaration, CaseResult, EnvironmentRecord,
    EnvironmentStatus, DECLARATION_SCHEMA, ENVIRONMENT_SCHEMA,
};

fn value(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn emit(markdown: String, output: Option<&PathBuf>) {
    if let Some(path) = output {
        std::fs::write(path, markdown)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    } else {
        print!("{markdown}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let declarations = value(&args, "--declarations")
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("missing --declarations"));
    let results = value(&args, "--results")
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("missing --results"));
    let output = value(&args, "--output").map(PathBuf::from);
    let artifact_root = value(&args, "--artifact-root").map(PathBuf::from);
    let environment = value(&args, "--environment").map(PathBuf::from);
    let require_video = args.iter().any(|arg| arg == "--require-video");

    let declarations: Vec<CaseDeclaration> = read_json_lines(&declarations)
        .unwrap_or_else(|errors| panic!("invalid declarations:\n{}", errors.join("\n")));
    let declarations = declarations
        .into_iter()
        .map(|declaration| {
            assert_eq!(
                declaration.schema, DECLARATION_SCHEMA,
                "unsupported declaration schema"
            );
            declaration.case
        })
        .collect::<Vec<_>>();
    let results: Vec<CaseResult> = read_json_lines(&results)
        .unwrap_or_else(|errors| panic!("invalid results:\n{}", errors.join("\n")));
    if let Some(path) = environment {
        let records: Vec<EnvironmentRecord> = read_json_lines(&path)
            .unwrap_or_else(|errors| panic!("invalid environment:\n{}", errors.join("\n")));
        assert_eq!(records.len(), 1, "expected one environment record");
        assert_eq!(
            records[0].schema, ENVIRONMENT_SCHEMA,
            "unsupported environment schema"
        );
        if records[0].status == EnvironmentStatus::Error {
            emit(
                format!(
                    "# CUA Driver E2E\n\n**Environment:** ERROR\n\n{}\n",
                    records[0].message
                ),
                output.as_ref(),
            );
            eprintln!("E2E environment is not ready: {}", records[0].message);
            std::process::exit(2);
        }
    }
    let summary = validate_catalog(
        &declarations,
        &results,
        artifact_root.as_deref(),
        require_video,
    )
    .unwrap_or_else(|errors| panic!("invalid E2E report:\n{}", errors.join("\n")));
    let markdown = summary.markdown_with_declarations(&declarations, &results);
    emit(markdown, output.as_ref());
}
