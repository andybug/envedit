//use clap::{Command, Arg};
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Seek, Write};
use std::process::Command;
use tempfile::NamedTempFile;

#[derive(Debug)]
struct EnvEditError {
    msg: String,
}

impl Error for EnvEditError {}

impl fmt::Display for EnvEditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl EnvEditError {
    fn new(msg: &str) -> EnvEditError {
        EnvEditError {
            msg: String::from(msg),
        }
    }
}

struct EnvVar {
    name: String,
    value: String,
}

impl EnvVar {
    fn validate_name(name: &str) -> Result<(), EnvEditError> {
        // the only restriction on environment variable names is that they
        // cannot have '=' in them
        match name.find('=') {
            Some(_) => Err(EnvEditError::new(
                "Variable name contains illegal character '='",
            )),
            None => Ok(()),
        }
    }

    pub fn new(name: String, value: String) -> Result<EnvVar, EnvEditError> {
        EnvVar::validate_name(name.as_str())?;
        Ok(EnvVar {
            name: name,
            value: value,
        })
    }
}

struct EnvVars(Vec<EnvVar>);

impl EnvVars {
    fn default() -> EnvVars {
        EnvVars { 0: Vec::new() }
    }

    fn insert(&mut self, var: EnvVar) {
        self.0.push(var);
    }

    fn sort(&mut self) {
        self.0.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

impl TryFrom<&mut dyn Iterator<Item = (String, String)>> for EnvVars {
    type Error = EnvEditError;

    fn try_from(vars: &mut dyn Iterator<Item = (String, String)>) -> Result<Self, Self::Error> {
        let mut env_vars = EnvVars::default();
        for var in vars {
            let env_var = EnvVar::new(var.0, var.1)?;
            env_vars.insert(env_var);
        }

        env_vars.sort();
        Ok(env_vars)
    }
}

impl TryFrom<&mut dyn Read> for EnvVars {
    type Error = EnvEditError;

    fn try_from(file: &mut dyn Read) -> Result<Self, Self::Error> {
        let mut env_vars = EnvVars::default();

        let reader = BufReader::new(file);
        for (index, line) in reader.lines().enumerate() {
            match line {
                Ok(s) => {
                    let v: Vec<&str> = s.split('=').collect();
                    if v.len() < 2 {
                        return Err(EnvEditError::new(&format!(
                            "Error reading file: line {} is malformed; missing '=' separator",
                            index
                        )));
                    }
                    let var = EnvVar::new(String::from(v[0]), String::from(v[1]))?;
                    env_vars.insert(var);
                }
                Err(e) => {
                    return Err(EnvEditError::new(
                        format!("Error reading temp file: {}", e).as_str(),
                    ))
                }
            }
        }

        env_vars.sort();
        Ok(env_vars)
    }
}

impl IntoIterator for EnvVars {
    type Item = EnvVar;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

enum DiffState {
    Unchanged,
    Modified,
    Added,
    Deleted,
}

struct DiffEntry {
    name: String,
    state: DiffState,
    old_value: Option<String>,
    new_value: Option<String>,
}

fn diff(old: EnvVars, new: EnvVars) -> Vec<DiffEntry> {
    let mut map = HashMap::new();

    for var in new {
        let entry = DiffEntry {
            name: String::from(&var.name),
            state: DiffState::Added,
            old_value: None,
            new_value: Some(String::from(var.value)),
        };
        map.insert(String::from(&var.name), entry);
    }

    for var in old {
        match map.get_mut(&var.name) {
            Some(mut entry) => {
                entry.old_value = Some(String::from(&var.value));
                if var.value == entry.new_value.as_deref().unwrap() {
                    entry.state = DiffState::Unchanged;
                } else {
                    entry.state = DiffState::Modified;
                }
            }
            None => {
                let entry = DiffEntry {
                    name: String::from(&var.name),
                    state: DiffState::Deleted,
                    old_value: Some(String::from(var.value)),
                    new_value: None,
                };
                map.insert(String::from(var.name), entry);
            }
        }
    }

    let mut entries = Vec::new();
    for (_, value) in map {
        entries.push(value);
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

fn write_temp_file(vars: &EnvVars) -> io::Result<NamedTempFile> {
    let mut file = NamedTempFile::new()?;
    for var in vars.0.iter() {
        writeln!(file, "{}={}", var.name, var.value)?;
    }
    file.flush()?;
    Ok(file)
}

fn main() {
    let env_vars =
        EnvVars::try_from(&mut env::vars() as &mut dyn Iterator<Item = (String, String)>)
            .expect("Failed to load variables from environment");

    let mut file = write_temp_file(&env_vars).expect("FIXME");
    let path = OsString::from(&file.path());

    let mut child = Command::new("nvim") // cspell:disable-line
        .arg(path)
        .arg("-c")
        .arg("set filetype=sh")
        .spawn()
        .expect("what on earth");

    child.wait().expect("wait");

    file.rewind().expect("yup");
    let edited_env_vars = EnvVars::try_from(&mut file as &mut dyn Read).expect("idk lol");

    let diff = diff(env_vars, edited_env_vars);

    for entry in diff {
        match entry.state {
            DiffState::Added => {
                println!("+ {}={}", entry.name, entry.new_value.unwrap());
            }
            DiffState::Deleted => {
                println!("- {}={}", entry.name, entry.old_value.unwrap());
            }
            DiffState::Modified => {
                println!("- {}={}", entry.name, entry.old_value.unwrap());
                println!("+ {}={}", entry.name, entry.new_value.unwrap());
            }
            DiffState::Unchanged => {
                println!("  {}={}", entry.name, entry.new_value.unwrap());
            }
        }
    }

    // let matches = Command::new("envedit")
    //     .arg(Arg::new("var")
    //         .required(false)
    //         .help("name of environment variable to edit")
    //         .multiple_occurrences(true))
    //     .get_matches();

    // if let Some(var) = matches.value_of("var") {
    //     println!("var = '{}'", var);
    // }
    // let output = Command::new("env").output().expect("msg");
    // println!("{}", output.stdout);
}

#[cfg(test)]
mod tests {
    use crate::EnvVars;

    #[test]
    fn env_vars_values() {
        let values = vec![
            (String::from("KEY"), String::from("VALUE")),
            (String::from("MULTILINE"), String::from("abc\ndef\n")),
        ];
        let result = EnvVars::try_from(
            &mut values.into_iter() as &mut dyn Iterator<Item = (String, String)>
        )
        .unwrap();

        assert_eq!(result.0.len(), 2);

        assert_eq!(result.0[0].name, "KEY");
        assert_eq!(result.0[0].value, "VALUE");

        assert_eq!(result.0[1].name, "MULTILINE");
        assert_eq!(result.0[1].value, "abc\ndef\n");
    }
}
