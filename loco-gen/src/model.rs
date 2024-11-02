use std::{collections::HashMap, env::current_dir};

use chrono::Utc;
use duct::cmd;
use rrgen::RRgen;
use serde_json::json;

use super::{Error, Result};
use crate::get_mappings;

const MODEL_T: &str = include_str!("templates/model.t");
const MODEL_TEST_T: &str = include_str!("templates/model_test.t");

use super::{collect_messages, AppInfo};

/// skipping some fields from the generated models.
/// For example, the `created_at` and `updated_at` fields are automatically
/// generated by the Loco app and should be given
pub const IGNORE_FIELDS: &[&str] = &["created_at", "updated_at", "create_at", "update_at"];

pub fn generate(
    rrgen: &RRgen,
    name: &str,
    is_link: bool,
    migration_only: bool,
    fields: &[(String, String)],
    appinfo: &AppInfo,
) -> Result<String> {
    let pkg_name: &str = &appinfo.app_name;
    let ts = Utc::now();

    let mut columns = Vec::new();
    let mut references = Vec::new();
    for (fname, ftype) in fields {
        if IGNORE_FIELDS.contains(&fname.as_str()) {
            tracing::warn!(
                field = fname,
                "note that a redundant field was specified, it is already generated automatically"
            );
            continue;
        }
        if ftype == "references" {
            let fkey = format!("{fname}_id");
            columns.push((fkey.clone(), "integer"));
            // user, user_id
            references.push((fname.to_string(), fkey));
        } else if ftype.starts_with("references:") {
            let fkey = format!("{fname}_id");
            columns.push((fkey.clone(), "integer"));
            references.push((ftype["references:".len()..].to_string(), fkey));
        } else {
            let mappings = get_mappings();
            let schema_type = mappings.schema_field(ftype.as_str()).ok_or_else(|| {
                Error::Message(format!(
                    "type: {} not found. try any of: {:?}",
                    ftype,
                    mappings.schema_fields()
                ))
            })?;
            columns.push((fname.to_string(), schema_type.as_str()));
        }
    }

    let vars = json!({"name": name, "ts": ts, "pkg_name": pkg_name, "is_link": is_link, "columns": columns, "references": references});
    let res1 = rrgen.generate(MODEL_T, &vars)?;
    let res2 = rrgen.generate(MODEL_TEST_T, &vars)?;

    if !migration_only {
        let cwd = current_dir()?;
        let env_map: HashMap<_, _> = std::env::vars().collect();

        let _ = cmd!("cargo", "loco-tool", "db", "migrate",)
            .stderr_to_stdout()
            .dir(cwd.as_path())
            .full_env(&env_map)
            .run()
            .map_err(|err| {
                Error::Message(format!(
                    "failed to run loco db migration. error details: `{err}`",
                ))
            })?;
        let _ = cmd!("cargo", "loco-tool", "db", "entities",)
            .stderr_to_stdout()
            .dir(cwd.as_path())
            .full_env(&env_map)
            .run()
            .map_err(|err| {
                Error::Message(format!(
                    "failed to run loco db entities. error details: `{err}`",
                ))
            })?;
    }

    let messages = collect_messages(vec![res1, res2]);
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use std::{env, process::Command};

    use crate::{
        testutil::{self, assert_cargo_check, assert_file, assert_single_file_match},
        AppInfo,
    };

    fn with_new_app<F>(app_name: &str, f: F)
    where
        F: FnOnce(),
    {
        testutil::with_temp_dir(|previous, current| {
            let status = Command::new("loco")
                .args([
                    "new",
                    "-n",
                    app_name,
                    "-t",
                    "saas",
                    "--db",
                    "sqlite",
                    "--bg",
                    "async",
                    "--assets",
                    "serverside",
                ])
                .env("STARTERS_LOCAL_PATH", previous.join("../"))
                .status()
                .expect("cannot run command");

            assert!(status.success(), "Command failed: loco new -n {app_name}");
            env::set_current_dir(current.join(app_name))
                .expect("Failed to change directory to app");
            f(); // Execute the provided closure
        })
        .expect("temp dir setup");
    }

    #[test]
    fn test_can_generate_model() {
        let rrgen = rrgen::RRgen::default();
        with_new_app("saas", || {
            super::generate(
                &rrgen,
                "movies",
                false,
                true,
                &[("title".to_string(), "string".to_string())],
                &AppInfo {
                    app_name: "saas".to_string(),
                },
            )
            .expect("generate");
            assert_file("migration/src/lib.rs", |content| {
                content.assert_syntax();
                content.assert_regex_match("_movies::Migration");
            });
            let migration = assert_single_file_match("migration/src", ".*_movies.rs$");
            assert_file(migration.to_str().unwrap(), |content| {
                content.assert_syntax();
                content.assert_regex_match("Title");
            });
            assert_cargo_check();
        });
    }
}
