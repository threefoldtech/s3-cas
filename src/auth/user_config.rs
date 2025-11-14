use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// User credentials and configuration
#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub access_key: String,
    pub secret_key: String,
}

/// Configuration file structure for users.toml
#[derive(Debug, Clone, Deserialize)]
pub struct UsersConfig {
    pub users: HashMap<String, User>,
}

impl UsersConfig {
    /// Load users configuration from a TOML file
    ///
    /// # Arguments
    /// * `path` - Path to the users.toml file
    ///
    /// # Returns
    /// * `Result<UsersConfig, String>` - Parsed configuration or error
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read users config file: {}", e))?;

        toml::from_str(&content)
            .map_err(|e| format!("Failed to parse users config: {}", e))
    }
}

/// UserAuth provides access_key → user_id mapping
#[derive(Debug, Clone)]
pub struct UserAuth {
    key_to_user: HashMap<String, String>,
    users: HashMap<String, User>,
}

impl UserAuth {
    /// Create a new UserAuth from UsersConfig
    pub fn new(config: UsersConfig) -> Self {
        let mut key_to_user = HashMap::new();

        for (user_id, user) in &config.users {
            key_to_user.insert(user.access_key.clone(), user_id.clone());
        }

        Self {
            key_to_user,
            users: config.users,
        }
    }

    /// Get user_id from access_key
    ///
    /// # Arguments
    /// * `access_key` - S3 access key from request
    ///
    /// # Returns
    /// * `Option<&str>` - User ID if found
    pub fn get_user_id(&self, access_key: &str) -> Option<&str> {
        self.key_to_user.get(access_key).map(|s| s.as_str())
    }

    /// Get user by user_id
    pub fn get_user(&self, user_id: &str) -> Option<&User> {
        self.users.get(user_id)
    }

    /// Get all user IDs
    pub fn user_ids(&self) -> impl Iterator<Item = &String> {
        self.users.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_users_config() {
        let toml_content = r#"
[users.alice]
access_key = "AKIA_ALICE"
secret_key = "secret_alice"

[users.bob]
access_key = "AKIA_BOB"
secret_key = "secret_bob"
"#;

        let config: UsersConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.users.len(), 2);
        assert_eq!(config.users.get("alice").unwrap().access_key, "AKIA_ALICE");
    }

    #[test]
    fn test_user_auth() {
        let mut users = HashMap::new();
        users.insert(
            "alice".to_string(),
            User {
                access_key: "AKIA_ALICE".to_string(),
                secret_key: "secret_alice".to_string(),
            },
        );

        let config = UsersConfig { users };
        let auth = UserAuth::new(config);

        assert_eq!(auth.get_user_id("AKIA_ALICE"), Some("alice"));
        assert_eq!(auth.get_user_id("UNKNOWN"), None);
    }
}
