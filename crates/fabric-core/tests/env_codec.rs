//! The environment spec codec: any valid spec, `secret_patterns`
//! included, decodes to itself, and its identity is a function of its content.

use std::collections::BTreeMap;

use fabric_core::env::{
    EnvSpec, EnvVarName, RunWhen, SetupArg, SetupStep, Tool, ToolName, ToolSource, ToolVersion,
};
use fabric_core::harness::ProgramName;
use fabric_core::path::RelPath;
use fabric_core::secret::SecretPattern;
use proptest::prelude::*;

fn rel_path() -> impl Strategy<Value = RelPath> {
    "[a-z_]{1,8}(/[a-z_.]{1,8}){0,2}"
        .prop_filter_map("not a relative path", |text| RelPath::parse(&text).ok())
}

fn tool() -> impl Strategy<Value = (ToolName, Tool)> {
    (
        "[a-z][a-z0-9:@/._-]{0,20}",
        "[ -~]{1,30}",
        prop_oneof![Just(ToolSource::Detected), Just(ToolSource::Declared)],
    )
        .prop_filter_map("not a pin", |(name, version, source)| {
            let name = ToolName::parse(&name).ok()?;
            let version = ToolVersion::parse(&version).ok()?;
            Some((
                name.clone(),
                Tool {
                    name,
                    version,
                    source,
                },
            ))
        })
}

fn step() -> impl Strategy<Value = SetupStep> {
    (
        "[a-z][a-z0-9-]{0,10}",
        prop::collection::vec(".{0,12}", 0..4),
        prop::option::of(rel_path()),
        prop_oneof![Just(RunWhen::CachesMissing), Just(RunWhen::Always)],
    )
        .prop_filter_map("not a program name", |(program, args, cwd, when)| {
            Some(SetupStep {
                program: ProgramName::parse(&program).ok()?,
                args: args
                    .iter()
                    .map(|arg| SetupArg::parse(arg).ok())
                    .collect::<Option<_>>()?,
                cwd,
                when,
            })
        })
}

fn spec() -> impl Strategy<Value = EnvSpec> {
    (
        prop::collection::btree_map("[a-z]", tool(), 0..4),
        prop::collection::vec(step(), 0..4),
        prop::collection::btree_set(
            "[A-Z_][A-Z0-9_]{0,12}".prop_filter_map("not a name", |t| EnvVarName::parse(&t).ok()),
            0..4,
        ),
        prop::collection::btree_set(rel_path(), 0..4),
        prop::collection::btree_set(
            "[a-z*.]{1,8}(/[a-z*.]{1,8})?"
                .prop_filter_map("not a pattern", |t| SecretPattern::parse(&t).ok()),
            0..4,
        ),
    )
        .prop_map(
            |(tools, setup, env_var_names, cache_paths, secret_patterns)| {
                let tools: BTreeMap<ToolName, Tool> = tools.into_values().collect();
                EnvSpec {
                    tools: tools.into_values().collect(),
                    setup,
                    env_var_names: env_var_names.into_iter().collect(),
                    cache_paths: cache_paths.into_iter().collect(),
                    secret_patterns: secret_patterns.into_iter().collect(),
                }
            },
        )
}

proptest! {
    #[test]
    fn a_valid_spec_decodes_to_itself(spec in spec()) {
        let bytes = spec.encode().unwrap();
        let decoded = EnvSpec::decode(&bytes).unwrap();
        prop_assert_eq!(&decoded, &spec);
        prop_assert_eq!(decoded.id().unwrap(), spec.id().unwrap());
    }
}
