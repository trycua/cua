use std::path::PathBuf;

use cua_driver_testkit::e2e::{
    environment_schema_supported, read_json_lines, validate_catalog, CaseDeclaration, CaseResult,
    EnvironmentRecord, EnvironmentStatus, DECLARATION_SCHEMA,
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

fn print_help() {
    println!(
        "cua-e2e-report\n\n\
Usage:\n  cua-e2e-report --declarations <jsonl> --results <jsonl> [OPTIONS]\n\n\
Options:\n  --artifact-root <dir>  Resolve and validate linked evidence artifacts\n  --environment <jsonl>  Include the runner environment record\n  --output <path>        Write Markdown to a file instead of stdout\n  --require-video        Require valid trajectory video evidence\n  -h, --help             Print this help"
    );
}

fn canonical_environment(records: Vec<EnvironmentRecord>) -> EnvironmentRecord {
    let mut records = records.into_iter();
    let first = records.next().expect("expected an environment record");
    for record in records {
        assert_eq!(
            record, first,
            "conflicting environment records in one E2E report"
        );
    }
    first
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return;
    }
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
    let mut source_sha = None;
    let mut environment_record = None;
    if let Some(path) = environment {
        let records: Vec<EnvironmentRecord> = read_json_lines(&path)
            .unwrap_or_else(|errors| panic!("invalid environment:\n{}", errors.join("\n")));
        for record in &records {
            assert!(
                environment_schema_supported(&record.schema),
                "unsupported environment schema: {}",
                record.schema,
            );
        }
        let record = canonical_environment(records);
        source_sha = record.source_sha.clone();
        if record.status == EnvironmentStatus::Error {
            emit(
                format!(
                    "# CUA Driver E2E\n\n**Environment:** ERROR\n\n{}\n",
                    record.message
                ),
                output.as_ref(),
            );
            eprintln!("E2E environment is not ready: {}", record.message);
            std::process::exit(2);
        }
        environment_record = Some(record);
    }
    let summary = validate_catalog(
        &declarations,
        &results,
        artifact_root.as_deref(),
        require_video,
    )
    .unwrap_or_else(|errors| panic!("invalid E2E report:\n{}", errors.join("\n")));
    let markdown = summary.markdown_with_declarations_source_and_environment(
        &declarations,
        &results,
        source_sha.as_deref(),
        environment_record.as_ref(),
    );
    emit(markdown, output.as_ref());
}

#[cfg(test)]
mod tests {
    use super::canonical_environment;
    use cua_driver_testkit::e2e::EnvironmentRecord;
    use std::time::Duration;

    #[test]
    fn identical_environment_records_collapse_to_one() {
        let record = EnvironmentRecord::ready(Duration::ZERO);
        assert_eq!(
            canonical_environment(vec![record.clone(), record.clone()]),
            record
        );
    }

    #[test]
    #[should_panic(expected = "conflicting environment records")]
    fn conflicting_environment_records_are_rejected() {
        let ready = EnvironmentRecord::ready(Duration::ZERO);
        let error = EnvironmentRecord::error(Duration::ZERO, "desktop unavailable");
        let _ = canonical_environment(vec![ready, error]);
    }
}
