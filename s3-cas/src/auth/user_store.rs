use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

use cas_storage::{MetaError, Store};

const USERS_TREE: &str = "_USERS";
const USERS_BY_S3_KEY_TREE: &str = "_USERS_BY_S3_KEY";

/// User record stored in the database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecord {
    /// Primary key - unique user identifier (e.g., "delandtj")
    pub user_id: String,
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
    pub fn new(
        user_id: String,
        s3_access_key: String,
        s3_secret_key: String,
        is_admin: bool,
    ) -> Result<Self, MetaError> {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| MetaError::OtherDBError(format!("System time error: {}", e)))?
            .as_secs();

        Ok(Self {
            user_id,
            s3_access_key,
            s3_secret_key,
            is_admin,
            created_at,
        })
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, MetaError> {
        serde_json::to_vec(self)
            .map_err(|e| MetaError::OtherDBError(format!("Failed to serialize UserRecord: {}", e)))
    }

    pub fn from_slice(data: &[u8]) -> Result<Self, MetaError> {
        serde_json::from_slice(data).map_err(|e| {
            MetaError::OtherDBError(format!("Failed to deserialize UserRecord: {}", e))
        })
    }
}

/// User store managing user records and S3-credential lookup
pub struct UserStore {
    store: Arc<dyn Store>,
}

impl UserStore {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    pub fn create_user(&self, user: UserRecord) -> Result<(), MetaError> {
        debug!("Creating user: {}", user.user_id);

        if self.get_user_by_id(&user.user_id)?.is_some() {
            return Err(MetaError::OtherDBError(format!(
                "User with ID '{}' already exists",
                user.user_id
            )));
        }

        if self.get_user_by_s3_key(&user.s3_access_key)?.is_some() {
            return Err(MetaError::OtherDBError(format!(
                "User with S3 access key '{}' already exists",
                user.s3_access_key
            )));
        }

        let user_data = user.to_vec()?;

        let users_tree = self.store.tree_open(USERS_TREE)?;
        users_tree.insert(user.user_id.as_bytes(), user_data)?;

        let s3_key_tree = self.store.tree_open(USERS_BY_S3_KEY_TREE)?;
        s3_key_tree.insert(user.s3_access_key.as_bytes(), user.user_id.as_bytes().to_vec())?;

        debug!("User created successfully: {}", user.user_id);
        Ok(())
    }

    pub fn get_user_by_id(&self, user_id: &str) -> Result<Option<UserRecord>, MetaError> {
        let users_tree = self.store.tree_open(USERS_TREE)?;
        match users_tree.get(user_id.as_bytes())? {
            Some(data) => Ok(Some(UserRecord::from_slice(&data)?)),
            None => Ok(None),
        }
    }

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

    pub fn list_users(&self) -> Result<Vec<UserRecord>, MetaError> {
        let users_tree = self.store.tree_ext_open(USERS_TREE)?;
        let mut users = Vec::new();

        for item in users_tree.iter_all() {
            let (_key, value) = item?;
            users.push(UserRecord::from_slice(&value)?);
        }

        Ok(users)
    }

    pub fn delete_user(&self, user_id: &str) -> Result<(), MetaError> {
        debug!("Deleting user: {}", user_id);

        let user = match self.get_user_by_id(user_id)? {
            Some(u) => u,
            None => {
                warn!("Attempted to delete non-existent user: {}", user_id);
                return Err(MetaError::OtherDBError(format!("User '{}' not found", user_id)));
            }
        };

        let users_tree = self.store.tree_open(USERS_TREE)?;
        users_tree.remove(user_id.as_bytes())?;

        let s3_key_tree = self.store.tree_open(USERS_BY_S3_KEY_TREE)?;
        s3_key_tree.remove(user.s3_access_key.as_bytes())?;

        debug!("User deleted successfully: {}", user_id);
        Ok(())
    }

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

    pub fn count_users(&self) -> Result<usize, MetaError> {
        let users_tree = self.store.tree_ext_open(USERS_TREE)?;
        Ok(users_tree.iter_all().count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_record_serialization() {
        let user = UserRecord::new(
            "testuser".to_string(),
            "AKIAIOSFODNN7EXAMPLE".to_string(),
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            true,
        )
        .unwrap();

        let serialized = user.to_vec().unwrap();
        let deserialized = UserRecord::from_slice(&serialized).unwrap();

        assert_eq!(user.user_id, deserialized.user_id);
        assert_eq!(user.s3_access_key, deserialized.s3_access_key);
        assert_eq!(user.is_admin, deserialized.is_admin);
    }
}
