use bcrypt::{hash, verify, DEFAULT_COST};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, warn};

use crate::metastore::{MetaError, Store};


const USERS_TREE: &str = "_USERS";
const USERS_BY_LOGIN_TREE: &str = "_USERS_BY_LOGIN";
const USERS_BY_S3_KEY_TREE: &str = "_USERS_BY_S3_KEY";

/// User record stored in the database
#[derive(Debug, Clone, Serialize, Deserialize, bincode::Encode, bincode::Decode)]
pub struct UserRecord {
    /// Primary key - unique user identifier (e.g., "delandtj")
    pub user_id: String,
    /// Username for HTTP UI login
    pub ui_login: String,
    /// Bcrypt password hash for UI authentication
    pub ui_password_hash: String,
    /// S3 access key (AWS format)
    pub s3_access_key: String,
    /// S3 secret key
    pub s3_secret_key: String,
    /// Whether user has admin privileges
    pub is_admin: bool,
    /// Account creation timestamp (seconds since UNIX epoch)
    pub created_at: u64,
}

impl UserRecord {
    /// Creates a new user record with bcrypt-hashed password
    pub fn new(
        user_id: String,
        ui_login: String,
        ui_password: &str,
        s3_access_key: String,
        s3_secret_key: String,
        is_admin: bool,
    ) -> Result<Self, MetaError> {
        let ui_password_hash = hash(ui_password, DEFAULT_COST)
            .map_err(|e| MetaError::OtherDBError(format!("Failed to hash password: {}", e)))?;

        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| MetaError::OtherDBError(format!("System time error: {}", e)))?
            .as_secs();

        Ok(Self {
            user_id,
            ui_login,
            ui_password_hash,
            s3_access_key,
            s3_secret_key,
            is_admin,
            created_at,
        })
    }

    /// Verifies a password against the stored hash
    pub fn verify_password(&self, password: &str) -> bool {
        match verify(password, &self.ui_password_hash) {
            Ok(valid) => valid,
            Err(e) => {
                error!("Password verification error: {}", e);
                false
            }
        }
    }

    /// Serializes the user record to bytes
    pub fn to_vec(&self) -> Result<Vec<u8>, MetaError> {
        bincode::encode_to_vec(self, bincode::config::standard())
            .map_err(|e| MetaError::OtherDBError(format!("Failed to serialize UserRecord: {}", e)))
    }

    /// Deserializes a user record from bytes
    pub fn from_slice(data: &[u8]) -> Result<Self, MetaError> {
        let (user, _len) = bincode::decode_from_slice(data, bincode::config::standard())
            .map_err(|e| MetaError::OtherDBError(format!("Failed to deserialize UserRecord: {}", e)))?;
        Ok(user)
    }

    /// Updates the password hash
    pub fn set_password(&mut self, new_password: &str) -> Result<(), MetaError> {
        self.ui_password_hash = hash(new_password, DEFAULT_COST)
            .map_err(|e| MetaError::OtherDBError(format!("Failed to hash password: {}", e)))?;
        Ok(())
    }
}

/// User store managing user authentication and metadata
pub struct UserStore {
    store: Arc<dyn Store>,
}

impl UserStore {
    /// Creates a new user store
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Creates a new user
    pub fn create_user(&self, user: UserRecord) -> Result<(), MetaError> {
        debug!("Creating user: {}", user.user_id);

        // Check if user_id already exists
        if self.get_user_by_id(&user.user_id)?.is_some() {
            return Err(MetaError::OtherDBError(format!(
                "User with ID '{}' already exists",
                user.user_id
            )));
        }

        // Check if ui_login already exists
        if self.get_user_by_ui_login(&user.ui_login)?.is_some() {
            return Err(MetaError::OtherDBError(format!(
                "User with login '{}' already exists",
                user.ui_login
            )));
        }

        // Check if s3_access_key already exists
        if self.get_user_by_s3_key(&user.s3_access_key)?.is_some() {
            return Err(MetaError::OtherDBError(format!(
                "User with S3 access key '{}' already exists",
                user.s3_access_key
            )));
        }

        let user_data = user.to_vec()?;

        // Store user by user_id (primary key)
        let users_tree = self.store.tree_open(USERS_TREE)?;
        users_tree.insert(user.user_id.as_bytes(), user_data)?;

        // Create index: ui_login -> user_id
        let login_tree = self.store.tree_open(USERS_BY_LOGIN_TREE)?;
        login_tree.insert(user.ui_login.as_bytes(), user.user_id.as_bytes().to_vec())?;

        // Create index: s3_access_key -> user_id
        let s3_key_tree = self.store.tree_open(USERS_BY_S3_KEY_TREE)?;
        s3_key_tree.insert(user.s3_access_key.as_bytes(), user.user_id.as_bytes().to_vec())?;

        debug!("User created successfully: {}", user.user_id);
        Ok(())
    }

    /// Gets a user by user_id
    pub fn get_user_by_id(&self, user_id: &str) -> Result<Option<UserRecord>, MetaError> {
        let users_tree = self.store.tree_open(USERS_TREE)?;
        match users_tree.get(user_id.as_bytes())? {
            Some(data) => Ok(Some(UserRecord::from_slice(&data)?)),
            None => Ok(None),
        }
    }

    /// Gets a user by UI login
    pub fn get_user_by_ui_login(&self, ui_login: &str) -> Result<Option<UserRecord>, MetaError> {
        let login_tree = self.store.tree_open(USERS_BY_LOGIN_TREE)?;
        match login_tree.get(ui_login.as_bytes())? {
            Some(user_id_bytes) => {
                let user_id = String::from_utf8(user_id_bytes.to_vec())
                    .map_err(|e| MetaError::OtherDBError(format!("Invalid UTF-8 in user_id: {}", e)))?;
                self.get_user_by_id(&user_id)
            }
            None => Ok(None),
        }
    }

    /// Gets a user by S3 access key
    pub fn get_user_by_s3_key(&self, s3_access_key: &str) -> Result<Option<UserRecord>, MetaError> {
        let s3_key_tree = self.store.tree_open(USERS_BY_S3_KEY_TREE)?;
        match s3_key_tree.get(s3_access_key.as_bytes())? {
            Some(user_id_bytes) => {
                let user_id = String::from_utf8(user_id_bytes.to_vec())
                    .map_err(|e| MetaError::OtherDBError(format!("Invalid UTF-8 in user_id: {}", e)))?;
                self.get_user_by_id(&user_id)
            }
            None => Ok(None),
        }
    }

    /// Lists all users
    pub fn list_users(&self) -> Result<Vec<UserRecord>, MetaError> {
        let users_tree = self.store.tree_ext_open(USERS_TREE)?;
        let mut users = Vec::new();

        for item in users_tree.iter_all() {
            let (_key, value) = item?;
            users.push(UserRecord::from_slice(&value)?);
        }

        Ok(users)
    }

    /// Deletes a user
    pub fn delete_user(&self, user_id: &str) -> Result<(), MetaError> {
        debug!("Deleting user: {}", user_id);

        // Get user to retrieve indexed fields
        let user = match self.get_user_by_id(user_id)? {
            Some(u) => u,
            None => {
                warn!("Attempted to delete non-existent user: {}", user_id);
                return Err(MetaError::OtherDBError(format!("User '{}' not found", user_id)));
            }
        };

        // Delete from primary tree
        let users_tree = self.store.tree_open(USERS_TREE)?;
        users_tree.remove(user_id.as_bytes())?;

        // Delete from login index
        let login_tree = self.store.tree_open(USERS_BY_LOGIN_TREE)?;
        login_tree.remove(user.ui_login.as_bytes())?;

        // Delete from S3 key index
        let s3_key_tree = self.store.tree_open(USERS_BY_S3_KEY_TREE)?;
        s3_key_tree.remove(user.s3_access_key.as_bytes())?;

        debug!("User deleted successfully: {}", user_id);
        Ok(())
    }

    /// Updates a user's password
    pub fn update_password(&self, user_id: &str, new_password: &str) -> Result<(), MetaError> {
        debug!("Updating password for user: {}", user_id);

        let mut user = match self.get_user_by_id(user_id)? {
            Some(u) => u,
            None => {
                return Err(MetaError::OtherDBError(format!("User '{}' not found", user_id)));
            }
        };

        user.set_password(new_password)?;

        let users_tree = self.store.tree_open(USERS_TREE)?;
        users_tree.insert(user_id.as_bytes(), user.to_vec()?)?;

        debug!("Password updated successfully for user: {}", user_id);
        Ok(())
    }

    /// Updates a user's admin status
    pub fn update_admin_status(&self, user_id: &str, is_admin: bool) -> Result<(), MetaError> {
        debug!("Updating admin status for user: {} to {}", user_id, is_admin);

        let mut user = match self.get_user_by_id(user_id)? {
            Some(u) => u,
            None => {
                return Err(MetaError::OtherDBError(format!("User '{}' not found", user_id)));
            }
        };

        user.is_admin = is_admin;

        let users_tree = self.store.tree_open(USERS_TREE)?;
        users_tree.insert(user_id.as_bytes(), user.to_vec()?)?;

        debug!("Admin status updated successfully for user: {}", user_id);
        Ok(())
    }

    /// Verifies a password for a user
    pub fn verify_password(&self, user_id: &str, password: &str) -> Result<bool, MetaError> {
        match self.get_user_by_id(user_id)? {
            Some(user) => Ok(user.verify_password(password)),
            None => Ok(false),
        }
    }

    /// Authenticates a user with UI login and password
    pub fn authenticate(&self, ui_login: &str, password: &str) -> Result<Option<UserRecord>, MetaError> {
        match self.get_user_by_ui_login(ui_login)? {
            Some(user) => {
                if user.verify_password(password) {
                    Ok(Some(user))
                } else {
                    debug!("Authentication failed for user: {} (invalid password)", ui_login);
                    Ok(None)
                }
            }
            None => {
                debug!("Authentication failed: user not found: {}", ui_login);
                Ok(None)
            }
        }
    }

    /// Counts the number of users
    pub fn count_users(&self) -> Result<usize, MetaError> {
        self.store.num_keys(USERS_TREE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_record_password_verification() {
        let user = UserRecord::new(
            "testuser".to_string(),
            "testlogin".to_string(),
            "password123",
            "AKIAIOSFODNN7EXAMPLE".to_string(),
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            false,
        )
        .unwrap();

        assert!(user.verify_password("password123"));
        assert!(!user.verify_password("wrongpassword"));
    }

    #[test]
    fn test_user_record_serialization() {
        let user = UserRecord::new(
            "testuser".to_string(),
            "testlogin".to_string(),
            "password123",
            "AKIAIOSFODNN7EXAMPLE".to_string(),
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            true,
        )
        .unwrap();

        let serialized = user.to_vec().unwrap();
        let deserialized = UserRecord::from_slice(&serialized).unwrap();

        assert_eq!(user.user_id, deserialized.user_id);
        assert_eq!(user.ui_login, deserialized.ui_login);
        assert_eq!(user.s3_access_key, deserialized.s3_access_key);
        assert_eq!(user.is_admin, deserialized.is_admin);
    }
}
