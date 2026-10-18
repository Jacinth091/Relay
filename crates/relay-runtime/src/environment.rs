use crate::SanitizedEnvironment;

const PARENT_ENV_ALLOW_LIST: [&str; 34] = [
    "SystemRoot",
    "SYSTEMDRIVE",
    "windir",
    "TEMP",
    "TMP",
    "PATH",
    "PATHEXT",
    "COMSPEC",
    "USERPROFILE",
    "LOCALAPPDATA",
    "APPDATA",
    "HOME",
    "USER",
    "LANG",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "ProgramData",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "CommonProgramW6432",
    "PSModulePath",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_IDENTIFIER",
    "NUMBER_OF_PROCESSORS",
    "OS",
    "HOMEDRIVE",
    "HOMEPATH",
    "USERNAME",
    "USERDOMAIN",
    "COMPUTERNAME",
    "PUBLIC",
    "ALLUSERSPROFILE",
    "SystemDrive",
];

pub(crate) fn resolved_child_environment(
    sanitized: &SanitizedEnvironment,
) -> Vec<(String, String)> {
    let mut environment = PARENT_ENV_ALLOW_LIST
        .into_iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_owned(), value))
        })
        .collect::<Vec<_>>();
    environment.extend(
        sanitized
            .vars
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    environment
}
