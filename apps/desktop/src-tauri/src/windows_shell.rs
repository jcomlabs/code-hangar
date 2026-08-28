use hangar_core::ShellIntegrationStatus;

#[cfg(windows)]
mod platform {
    use super::ShellIntegrationStatus;
    use std::io;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::UI::Shell::{
        SHChangeNotify, ShellExecuteW, SHCNE_ASSOCCHANGED, SHCNF_IDLIST,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_NONE};
    use winreg::{RegKey, RegValue};

    const PREFERENCES_KEY: &str = r"Software\JCOM Labs\Code Hangar\ShellIntegration";
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const RUN_VALUE_NAME: &str = "Code Hangar";
    const CAPABILITIES_KEY: &str = r"Software\JCOM Labs\Code Hangar\Capabilities";
    const REGISTERED_APPLICATIONS_KEY: &str = r"Software\RegisteredApplications";
    const REGISTERED_APPLICATION_NAME: &str = "Code Hangar";
    const PROG_ID: &str = "CodeHangar.Markdown";
    const PROG_ID_KEY: &str = r"Software\Classes\CodeHangar.Markdown";
    const TEXT_CONTEXT_KEY: &str = r"Software\Classes\SystemFileAssociations\text\shell\CodeHangar";
    const DIRECTORY_CONTEXT_KEY: &str = r"Software\Classes\Directory\shell\CodeHangar";
    const DIRECTORY_BACKGROUND_KEY: &str =
        r"Software\Classes\Directory\Background\shell\CodeHangar";
    const MARKDOWN_EXTENSIONS: [&str; 3] = [".md", ".markdown", ".mdx"];

    pub fn status() -> Result<ShellIntegrationStatus, String> {
        let owner = read_string(PREFERENCES_KEY, "OwnerExecutable");
        let current = current_executable()?;
        let owned_by_current = owner
            .as_deref()
            .is_some_and(|value| same_windows_path(value, &current));
        let markdown_preference = read_dword(PREFERENCES_KEY, "MarkdownEnabled") == Some(1);
        let context_preference = read_dword(PREFERENCES_KEY, "ContextMenuEnabled") == Some(1);
        let markdown_registered = owner.as_deref().is_some_and(|executable| {
            markdown_preference && markdown_registration_matches(executable)
        });
        let context_menu_registered = owner.as_deref().is_some_and(|executable| {
            context_preference && context_registration_matches(executable)
        });
        let run_command = read_string(RUN_KEY, RUN_VALUE_NAME);
        let run_owner = run_command
            .as_deref()
            .and_then(background_run_owner)
            .map(ToString::to_string);
        let run_owned_by_current = run_owner
            .as_deref()
            .is_some_and(|value| same_windows_path(value, &current));

        Ok(ShellIntegrationStatus {
            available: true,
            markdown_registered,
            context_menu_registered,
            run_at_login: run_command.is_some(),
            run_at_login_owned_by_current_edition: run_owned_by_current,
            run_at_login_owner_executable: run_owner,
            background_refresh_enabled: true,
            close_to_tray: true,
            owned_by_current_edition: owned_by_current,
            default_guide_pending: markdown_registered
                && read_dword(PREFERENCES_KEY, "DefaultGuidePending") == Some(1),
            owner_executable: owner,
        })
    }

    pub fn set(markdown: bool, context_menu: bool) -> Result<ShellIntegrationStatus, String> {
        let previous = status()?;
        let executable = current_executable()?;
        if markdown {
            register_markdown(&executable)?;
        } else {
            unregister_markdown()?;
        }
        if context_menu {
            register_context_menu(&executable)?;
        } else {
            unregister_context_menu()?;
        }

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (preferences, _) = hkcu
            .create_subkey(PREFERENCES_KEY)
            .map_err(registry_error("write shell preferences"))?;
        preferences
            .set_value("MarkdownEnabled", &u32::from(markdown))
            .map_err(registry_error("save the Markdown preference"))?;
        preferences
            .set_value("ContextMenuEnabled", &u32::from(context_menu))
            .map_err(registry_error("save the context-menu preference"))?;
        if markdown || context_menu {
            preferences
                .set_value("OwnerExecutable", &executable)
                .map_err(registry_error("save the shell-integration owner"))?;
        } else {
            delete_value_if_present(&preferences, "OwnerExecutable")?;
        }
        let guide_pending =
            markdown && (previous.default_guide_pending || !previous.markdown_registered);
        preferences
            .set_value("DefaultGuidePending", &u32::from(guide_pending))
            .map_err(registry_error("save the Default Apps guide state"))?;

        notify_shell();
        status()
    }

    pub fn set_run_at_login(enabled: bool) -> Result<ShellIntegrationStatus, String> {
        let executable = current_executable()?;
        let expected_command = background_run_command(&executable);
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (run, _) = hkcu
            .create_subkey(RUN_KEY)
            .map_err(registry_error("open the current-user startup key"))?;
        let existing = run.get_value::<String, _>(RUN_VALUE_NAME).ok();
        let existing_owner = existing.as_deref().and_then(background_run_owner);
        let owned_by_current =
            existing_owner.is_some_and(|owner| same_windows_path(owner, &executable));
        if existing.is_some() && !owned_by_current {
            let owner = existing_owner.unwrap_or("another executable");
            return Err(format!(
                "Start with Windows is owned by {owner}. Open that Code Hangar edition to change it."
            ));
        }
        if enabled {
            run.set_value(RUN_VALUE_NAME, &expected_command)
                .map_err(registry_error("enable Code Hangar at sign-in"))?;
        } else if existing.is_some() {
            delete_value_if_present(&run, RUN_VALUE_NAME)?;
        }
        status()
    }

    pub fn dismiss_default_guide() -> Result<ShellIntegrationStatus, String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (preferences, _) = hkcu
            .create_subkey(PREFERENCES_KEY)
            .map_err(registry_error("open shell preferences"))?;
        preferences
            .set_value("DefaultGuidePending", &0_u32)
            .map_err(registry_error("dismiss the Default Apps guide"))?;
        status()
    }

    pub fn open_default_apps() -> Result<(), String> {
        let verb = wide("open");
        let target = wide("ms-settings:defaultapps");
        // SAFETY: all text buffers are NUL-terminated and live for the duration
        // of the call. The target is a fixed Windows Settings URI, never input.
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if result as isize <= 32 {
            return Err("Windows could not open Default Apps settings.".to_string());
        }
        Ok(())
    }

    fn register_markdown(executable: &str) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (registered, _) = hkcu
            .create_subkey(REGISTERED_APPLICATIONS_KEY)
            .map_err(registry_error("open Registered Applications"))?;
        registered
            .set_value(REGISTERED_APPLICATION_NAME, &CAPABILITIES_KEY)
            .map_err(registry_error("register Code Hangar capabilities"))?;

        let (capabilities, _) = hkcu
            .create_subkey(CAPABILITIES_KEY)
            .map_err(registry_error("create Code Hangar capabilities"))?;
        capabilities
            .set_value("ApplicationName", &"Code Hangar")
            .map_err(registry_error("write the registered application name"))?;
        capabilities
            .set_value(
                "ApplicationDescription",
                &"Local-first project and Markdown explorer",
            )
            .map_err(registry_error(
                "write the registered application description",
            ))?;
        let (associations, _) = capabilities
            .create_subkey("FileAssociations")
            .map_err(registry_error("create Markdown associations"))?;
        for extension in MARKDOWN_EXTENSIONS {
            associations
                .set_value(extension, &PROG_ID)
                .map_err(registry_error("register a Markdown extension"))?;
            let (open_with, _) = hkcu
                .create_subkey(format!(r"Software\Classes\{extension}\OpenWithProgids"))
                .map_err(registry_error("create a Markdown Open With entry"))?;
            open_with
                .set_raw_value(
                    PROG_ID,
                    &RegValue {
                        bytes: Vec::new(),
                        vtype: REG_NONE,
                    },
                )
                .map_err(registry_error("write a Markdown Open With entry"))?;
        }

        let (prog_id, _) = hkcu
            .create_subkey(PROG_ID_KEY)
            .map_err(registry_error("create the Markdown file type"))?;
        prog_id
            .set_value("", &"Markdown document")
            .map_err(registry_error("name the Markdown file type"))?;
        prog_id
            .set_value("FriendlyTypeName", &"Markdown document")
            .map_err(registry_error("name the Markdown file type"))?;
        let (icon, _) = prog_id
            .create_subkey("DefaultIcon")
            .map_err(registry_error("create the Markdown icon entry"))?;
        icon.set_value("", &format!("\"{executable}\",0"))
            .map_err(registry_error("write the Markdown icon entry"))?;
        let (command, _) = prog_id
            .create_subkey(r"shell\open\command")
            .map_err(registry_error("create the Markdown open command"))?;
        command
            .set_value("", &open_command(executable, "%1"))
            .map_err(registry_error("write the Markdown open command"))?;
        Ok(())
    }

    fn unregister_markdown() -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        delete_tree_if_present(&hkcu, PROG_ID_KEY)?;
        delete_tree_if_present(&hkcu, CAPABILITIES_KEY)?;
        if let Ok(registered) =
            hkcu.open_subkey_with_flags(REGISTERED_APPLICATIONS_KEY, KEY_READ | KEY_WRITE)
        {
            if registered
                .get_value::<String, _>(REGISTERED_APPLICATION_NAME)
                .ok()
                .as_deref()
                == Some(CAPABILITIES_KEY)
            {
                delete_value_if_present(&registered, REGISTERED_APPLICATION_NAME)?;
            }
        }
        for extension in MARKDOWN_EXTENSIONS {
            let path = format!(r"Software\Classes\{extension}\OpenWithProgids");
            if let Ok(open_with) = hkcu.open_subkey_with_flags(path, KEY_READ | KEY_WRITE) {
                delete_value_if_present(&open_with, PROG_ID)?;
            }
        }
        Ok(())
    }

    fn register_context_menu(executable: &str) -> Result<(), String> {
        register_context_command(TEXT_CONTEXT_KEY, "Open in Code Hangar", executable, "%1")?;
        for extension in MARKDOWN_EXTENSIONS {
            register_context_command(
                &format!(r"Software\Classes\SystemFileAssociations\{extension}\shell\CodeHangar"),
                "Open in Code Hangar",
                executable,
                "%1",
            )?;
        }
        register_context_command(
            DIRECTORY_CONTEXT_KEY,
            "Open folder in Code Hangar",
            executable,
            "%1",
        )?;
        register_context_command(
            DIRECTORY_BACKGROUND_KEY,
            "Open folder in Code Hangar",
            executable,
            "%V",
        )
    }

    fn register_context_command(
        path: &str,
        label: &str,
        executable: &str,
        placeholder: &str,
    ) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (menu, _) = hkcu
            .create_subkey(path)
            .map_err(registry_error("create a Code Hangar context-menu entry"))?;
        menu.set_value("", &label)
            .map_err(registry_error("write a context-menu label"))?;
        menu.set_value("Icon", &format!("\"{executable}\",0"))
            .map_err(registry_error("write a context-menu icon"))?;
        let (command, _) = menu
            .create_subkey("command")
            .map_err(registry_error("create a context-menu command"))?;
        command
            .set_value("", &open_command(executable, placeholder))
            .map_err(registry_error("write a context-menu command"))
    }

    fn unregister_context_menu() -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        delete_tree_if_present(&hkcu, TEXT_CONTEXT_KEY)?;
        for extension in MARKDOWN_EXTENSIONS {
            delete_tree_if_present(
                &hkcu,
                &format!(r"Software\Classes\SystemFileAssociations\{extension}\shell\CodeHangar"),
            )?;
        }
        delete_tree_if_present(&hkcu, DIRECTORY_CONTEXT_KEY)?;
        delete_tree_if_present(&hkcu, DIRECTORY_BACKGROUND_KEY)
    }

    fn markdown_registration_matches(executable: &str) -> bool {
        read_string(&format!(r"{PROG_ID_KEY}\shell\open\command"), "")
            .is_some_and(|value| value == open_command(executable, "%1"))
            && read_string(REGISTERED_APPLICATIONS_KEY, REGISTERED_APPLICATION_NAME).as_deref()
                == Some(CAPABILITIES_KEY)
            && MARKDOWN_EXTENSIONS.into_iter().all(|extension| {
                read_string(&format!(r"{CAPABILITIES_KEY}\FileAssociations"), extension).as_deref()
                    == Some(PROG_ID)
                    && registry_value_exists(
                        &format!(r"Software\Classes\{extension}\OpenWithProgids"),
                        PROG_ID,
                    )
            })
    }

    fn context_registration_matches(executable: &str) -> bool {
        [
            (TEXT_CONTEXT_KEY, "%1"),
            (DIRECTORY_CONTEXT_KEY, "%1"),
            (DIRECTORY_BACKGROUND_KEY, "%V"),
        ]
        .into_iter()
        .all(|(path, placeholder)| {
            read_string(&format!(r"{path}\command"), "")
                .is_some_and(|value| value == open_command(executable, placeholder))
        }) && MARKDOWN_EXTENSIONS.into_iter().all(|extension| {
            read_string(
                &format!(
                    r"Software\Classes\SystemFileAssociations\{extension}\shell\CodeHangar\command"
                ),
                "",
            )
            .is_some_and(|value| value == open_command(executable, "%1"))
        })
    }

    fn current_executable() -> Result<String, String> {
        let path = std::env::current_exe()
            .map_err(|err| format!("Cannot locate the Code Hangar executable: {err}"))?;
        Ok(display_windows_path(&path))
    }

    fn display_windows_path(path: &Path) -> String {
        let value = path.to_string_lossy();
        value.strip_prefix(r"\\?\").unwrap_or(&value).to_string()
    }

    fn same_windows_path(left: &str, right: &str) -> bool {
        let normalize = |value: &str| {
            display_windows_path(&PathBuf::from(value))
                .replace('/', "\\")
                .to_ascii_lowercase()
        };
        normalize(left) == normalize(right)
    }

    fn open_command(executable: &str, placeholder: &str) -> String {
        format!("\"{executable}\" \"{placeholder}\"")
    }

    fn background_run_command(executable: &str) -> String {
        format!("\"{executable}\" --background")
    }

    fn background_run_owner(command: &str) -> Option<&str> {
        command
            .strip_suffix(" --background")?
            .strip_prefix('"')?
            .strip_suffix('"')
            .filter(|value| !value.is_empty())
    }

    fn read_string(path: &str, name: &str) -> Option<String> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(path, KEY_READ)
            .ok()?
            .get_value(name)
            .ok()
    }

    fn read_dword(path: &str, name: &str) -> Option<u32> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(path, KEY_READ)
            .ok()?
            .get_value(name)
            .ok()
    }

    fn registry_value_exists(path: &str, name: &str) -> bool {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(path, KEY_READ)
            .ok()
            .and_then(|key| key.get_raw_value(name).ok())
            .is_some()
    }

    fn delete_tree_if_present(parent: &RegKey, path: &str) -> Result<(), String> {
        match parent.delete_subkey_all(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "Could not remove a Code Hangar registry key: {error}"
            )),
        }
    }

    fn delete_value_if_present(key: &RegKey, name: &str) -> Result<(), String> {
        match key.delete_value(name) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "Could not remove a Code Hangar registry value: {error}"
            )),
        }
    }

    fn registry_error(action: &'static str) -> impl FnOnce(io::Error) -> String {
        move |error| format!("Could not {action}: {error}")
    }

    fn notify_shell() {
        // SAFETY: SHCNE_ASSOCCHANGED with SHCNF_IDLIST requires two null item
        // pointers. This asks Explorer to refresh association caches only.
        unsafe {
            SHChangeNotify(
                SHCNE_ASSOCCHANGED as i32,
                SHCNF_IDLIST,
                std::ptr::null(),
                std::ptr::null(),
            );
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn commands_quote_both_executable_and_shell_placeholder() {
            assert_eq!(
                open_command(r"C:\Program Files\Code Hangar\Code Hangar.exe", "%1"),
                r#""C:\Program Files\Code Hangar\Code Hangar.exe" "%1""#
            );
            assert_eq!(
                background_run_command(r"C:\Program Files\Code Hangar\Code Hangar.exe"),
                r#""C:\Program Files\Code Hangar\Code Hangar.exe" --background"#
            );
        }

        #[test]
        fn ownership_comparison_is_windows_case_insensitive() {
            assert!(same_windows_path(
                r"C:\Apps\Code Hangar.exe",
                r"c:/apps/code hangar.exe"
            ));
        }

        #[test]
        fn startup_owner_is_parsed_only_from_the_exact_registered_shape() {
            assert_eq!(
                background_run_owner(
                    r#""C:\Program Files\Code Hangar\Code Hangar.exe" --background"#
                ),
                Some(r"C:\Program Files\Code Hangar\Code Hangar.exe")
            );
            assert_eq!(
                background_run_owner(r"C:\CodeHangar.exe --background"),
                None
            );
            assert_eq!(background_run_owner(r#""C:\CodeHangar.exe" --other"#), None);
        }
    }
}

#[cfg(windows)]
pub use platform::{dismiss_default_guide, open_default_apps, set, set_run_at_login, status};

#[cfg(not(windows))]
pub fn status() -> Result<ShellIntegrationStatus, String> {
    Ok(ShellIntegrationStatus {
        background_refresh_enabled: true,
        close_to_tray: true,
        ..ShellIntegrationStatus::default()
    })
}

#[cfg(not(windows))]
pub fn set(_markdown: bool, _context_menu: bool) -> Result<ShellIntegrationStatus, String> {
    Err("Windows shell integration is available only on Windows.".to_string())
}

#[cfg(not(windows))]
pub fn set_run_at_login(_enabled: bool) -> Result<ShellIntegrationStatus, String> {
    Err("Run at login is available only on Windows.".to_string())
}

#[cfg(not(windows))]
pub fn dismiss_default_guide() -> Result<ShellIntegrationStatus, String> {
    status()
}

#[cfg(not(windows))]
pub fn open_default_apps() -> Result<(), String> {
    Err("Windows Default Apps settings are available only on Windows.".to_string())
}
