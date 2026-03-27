#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use crate::app::{extract_ssh_credential_target, normalize_enter_mode};

    #[test]
    fn test_normalize_enter_mode() {
        let cases = [
            ("p", "p"),
            ("pane", "p"),
            ("P", "p"),
            ("o", "o"),
            ("origin", "o"),
            ("O", "o"),
            ("w", "w"),
            ("window", "w"),
            ("s", "s"),
            ("split", "s"),
            ("split-h", "s"),
            ("v", "v"),
            ("split-v", "v"),
            ("", "p"),
            ("junk", "p"),
        ];
        for (i, w) in cases {
            assert_eq!(normalize_enter_mode(i), w);
        }
    }

    #[test]
    fn test_extract_ssh_credential_target_user_at_host() {
        let target = extract_ssh_credential_target("ssh", &[OsString::from("matt@edge1")]).unwrap();
        assert_eq!(target.host, "edge1");
        assert_eq!(target.user, "matt");
    }

    #[test]
    fn test_extract_ssh_credential_target_user_flag() {
        let args = [
            OsString::from("-l"),
            OsString::from("matt"),
            OsString::from("edge1"),
        ];
        let target = extract_ssh_credential_target("ssh", &args).unwrap();
        assert_eq!(target.host, "edge1");
        assert_eq!(target.user, "matt");
    }

    #[test]
    fn test_extract_ssh_credential_target_option_user_flag() {
        let args = [
            OsString::from("-o"),
            OsString::from("User=matt"),
            OsString::from("edge1"),
        ];
        let target = extract_ssh_credential_target("ssh", &args).unwrap();
        assert_eq!(target.host, "edge1");
        assert_eq!(target.user, "matt");
    }

    #[test]
    fn test_extract_ssh_credential_target_for_scp() {
        let args = [
            OsString::from("file.txt"),
            OsString::from("matt@edge1:/tmp/file.txt"),
        ];
        let target = extract_ssh_credential_target("scp", &args).unwrap();
        assert_eq!(target.host, "edge1");
        assert_eq!(target.user, "matt");
    }

    #[test]
    fn test_connect_askpass_env_names_are_stable() {
        // This test documents the env contract used by the askpass script.
        // It is not a full integration test.
        let exe = std::env::current_exe().unwrap();
        let mut cmd = super::super::app::credential_command_for_path(
            &exe,
            "set",
            "narrs-dev4.lmig.com",
            "root",
            "password",
        );
        cmd.env("TSSM_HOST", "narrs-dev4.lmig.com");
        cmd.env("TSSM_USER", "root");
        assert!(cmd.get_envs().any(|(k, _)| k == "TSSM_HOST"));
        assert!(cmd.get_envs().any(|(k, _)| k == "TSSM_USER"));
    }
}
