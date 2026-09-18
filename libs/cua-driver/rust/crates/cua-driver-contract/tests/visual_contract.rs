// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use cua_driver_contract::{
    manifest, ParseVisualRegionsInput, ParseVisualRegionsOutput, VisualParseError,
    VisualParseErrorCode,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

fn round_trip_valid<T>(fixture: &str, validate: impl FnOnce(&T))
where
    T: DeserializeOwned + Serialize,
{
    let expected: Value = serde_json::from_str(fixture).expect("fixture is JSON");
    let value: T = serde_json::from_value(expected.clone()).expect("fixture matches the DTO");
    validate(&value);
    assert_eq!(serde_json::to_value(value).unwrap(), expected);
}

#[test]
fn versioned_request_fixtures_cover_minimal_and_full_options() {
    for fixture in [
        include_str!("fixtures/parse-visual-regions-request-minimal-v1.json"),
        include_str!("fixtures/parse-visual-regions-request-full-v1.json"),
    ] {
        round_trip_valid::<ParseVisualRegionsInput>(fixture, |input| input.validate().unwrap());
    }
}

#[test]
fn versioned_output_fixture_covers_provenance_regions_warnings_and_optionals() {
    round_trip_valid::<ParseVisualRegionsOutput>(
        include_str!("fixtures/parse-visual-regions-output-full-v1.json"),
        |output| {
            output.validate().unwrap();
            let (pixel_x, pixel_y) = output.regions[0].bounds.center();
            assert_eq!((pixel_x, pixel_y), (60.5, 30.5));
            assert_eq!(
                output.regions[0].action_center(&output.capture.action_coordinate_space),
                (-1859.5, 30.5)
            );
        },
    );
}

#[test]
fn versioned_error_fixture_exhausts_the_stable_v1_codes() {
    let fixture = include_str!("fixtures/visual-parse-errors-v1.json");
    let expected: Value = serde_json::from_str(fixture).unwrap();
    let errors: Vec<VisualParseError> = serde_json::from_value(expected.clone()).unwrap();
    for error in &errors {
        error.validate().unwrap();
    }
    assert_eq!(serde_json::to_value(&errors).unwrap(), expected);

    let actual: BTreeSet<_> = errors.iter().map(|error| error.code).collect();
    let all = BTreeSet::from([
        VisualParseErrorCode::NotInstalled,
        VisualParseErrorCode::CaptureNotFound,
        VisualParseErrorCode::CaptureExpired,
        VisualParseErrorCode::CaptureStale,
        VisualParseErrorCode::CaptureGenerationMismatch,
        VisualParseErrorCode::UnsupportedTarget,
        VisualParseErrorCode::UnsupportedPlatform,
        VisualParseErrorCode::IncompatibleProtocol,
        VisualParseErrorCode::InvalidFrame,
        VisualParseErrorCode::WorkerLaunchFailed,
        VisualParseErrorCode::WorkerCrashed,
        VisualParseErrorCode::WorkerCancelled,
        VisualParseErrorCode::Timeout,
        VisualParseErrorCode::ResourceLimitExceeded,
        VisualParseErrorCode::ArtifactInvalid,
        VisualParseErrorCode::InferenceFailed,
    ]);
    assert_eq!(actual, all);
}

#[test]
fn visual_contract_remains_an_inert_dto_foundation() {
    assert!(manifest()
        .tools
        .iter()
        .all(|tool| tool.name != "parse_visual_regions"));
}
