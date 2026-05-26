use crate::models::connection::{ConnectionConfig, DatabaseType};
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const MAIN_PASSWORD_KEY: &str = "password";
pub const SSH_PASSWORD_KEY: &str = "ssh_password";
pub const SSH_KEY_PASSPHRASE_KEY: &str = "ssh_key_passphrase";
pub const CONNECTION_STRING_KEY: &str = "connection_string";
pub const FEISHU_ACCESS_TOKEN_KEY: &str = "feishu_access_token";

pub trait ConnectionSecretStore {
    fn set_secret(&self, connection_id: &str, key: &str, secret: &str) -> Result<(), String>;
    fn get_secret(&self, connection_id: &str, key: &str) -> Result<Option<String>, String>;
    fn delete_secret(&self, connection_id: &str, key: &str) -> Result<(), String>;
}

pub struct FileSecretStore {
    path: PathBuf,
}

impl FileSecretStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn read_store(&self) -> HashMap<String, String> {
        std::fs::read_to_string(&self.path).ok().and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
    }

    fn write_store(&self, map: &HashMap<String, String>) -> Result<(), String> {
        let json = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, json).map_err(|e| e.to_string())
    }
}

impl ConnectionSecretStore for FileSecretStore {
    fn set_secret(&self, connection_id: &str, key: &str, secret: &str) -> Result<(), String> {
        let mut map = self.read_store();
        map.insert(secret_account(connection_id, key), secret.to_string());
        self.write_store(&map)
    }

    fn get_secret(&self, connection_id: &str, key: &str) -> Result<Option<String>, String> {
        Ok(self.read_store().get(&secret_account(connection_id, key)).cloned())
    }

    fn delete_secret(&self, connection_id: &str, key: &str) -> Result<(), String> {
        let mut map = self.read_store();
        map.remove(&secret_account(connection_id, key));
        self.write_store(&map)
    }
}

pub fn save_connections_to_file(
    path: &Path,
    configs: &[ConnectionConfig],
    store: &dyn ConnectionSecretStore,
) -> Result<(), String> {
    delete_removed_connection_secrets(path, configs, store)?;
    for config in configs {
        persist_secret(store, &config.id, MAIN_PASSWORD_KEY, &config.password)?;
        persist_secret(store, &config.id, SSH_PASSWORD_KEY, &config.ssh_password)?;
        persist_secret(store, &config.id, SSH_KEY_PASSPHRASE_KEY, &config.ssh_key_passphrase)?;
        persist_optional_secret(store, &config.id, CONNECTION_STRING_KEY, config.connection_string.as_deref())?;
        persist_optional_secret(store, &config.id, FEISHU_ACCESS_TOKEN_KEY, feishu_access_token(config).as_deref())?;
    }

    write_sanitized_connections(path, configs)
}

pub fn load_connections_from_file(
    path: &Path,
    store: &dyn ConnectionSecretStore,
) -> Result<Vec<ConnectionConfig>, String> {
    if !path.exists() {
        return Ok(vec![]);
    }

    let mut configs = read_connections(path)?;
    let mut needs_rewrite = false;
    for config in &mut configs {
        let legacy_feishu_token = feishu_access_token(config);
        if config.password.is_empty() {
            if let Some(secret) = store.get_secret(&config.id, MAIN_PASSWORD_KEY)? {
                config.password = secret;
            }
        } else {
            store.set_secret(&config.id, MAIN_PASSWORD_KEY, &config.password)?;
            needs_rewrite = true;
        }

        if config.ssh_password.is_empty() {
            if let Some(secret) = store.get_secret(&config.id, SSH_PASSWORD_KEY)? {
                config.ssh_password = secret;
            }
        } else {
            store.set_secret(&config.id, SSH_PASSWORD_KEY, &config.ssh_password)?;
            needs_rewrite = true;
        }

        if config.ssh_key_passphrase.is_empty() {
            if let Some(secret) = store.get_secret(&config.id, SSH_KEY_PASSPHRASE_KEY)? {
                config.ssh_key_passphrase = secret;
            }
        } else {
            store.set_secret(&config.id, SSH_KEY_PASSPHRASE_KEY, &config.ssh_key_passphrase)?;
            needs_rewrite = true;
        }

        match config.connection_string.as_deref().filter(|secret| !secret.is_empty()) {
            Some(secret) => {
                store.set_secret(&config.id, CONNECTION_STRING_KEY, secret)?;
                needs_rewrite = true;
            }
            None => {
                if let Some(secret) = store.get_secret(&config.id, CONNECTION_STRING_KEY)? {
                    config.connection_string = Some(secret);
                }
            }
        }

        let stored_feishu_token = store.get_secret(&config.id, FEISHU_ACCESS_TOKEN_KEY)?;
        if legacy_feishu_token.is_some() {
            needs_rewrite = true;
        }
        let feishu_token = match (stored_feishu_token, legacy_feishu_token) {
            (Some(token), _) => Some(token),
            (None, Some(token)) => {
                store.set_secret(&config.id, FEISHU_ACCESS_TOKEN_KEY, &token)?;
                Some(token)
            }
            (None, None) => None,
        };
        if let Some(token) = feishu_token {
            set_feishu_access_token(config, token);
        }
    }

    if needs_rewrite {
        write_sanitized_connections(path, &configs)?;
    }

    Ok(configs)
}

fn delete_removed_connection_secrets(
    path: &Path,
    configs: &[ConnectionConfig],
    store: &dyn ConnectionSecretStore,
) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    let previous = match read_connections(path) {
        Ok(configs) => configs,
        Err(_) => return Ok(()),
    };
    let current_ids: HashSet<&str> = configs.iter().map(|config| config.id.as_str()).collect();
    for config in previous {
        if current_ids.contains(config.id.as_str()) {
            continue;
        }
        store.delete_secret(&config.id, MAIN_PASSWORD_KEY)?;
        store.delete_secret(&config.id, SSH_PASSWORD_KEY)?;
        store.delete_secret(&config.id, SSH_KEY_PASSPHRASE_KEY)?;
        store.delete_secret(&config.id, CONNECTION_STRING_KEY)?;
        store.delete_secret(&config.id, FEISHU_ACCESS_TOKEN_KEY)?;
    }
    Ok(())
}

fn persist_secret(
    store: &dyn ConnectionSecretStore,
    connection_id: &str,
    key: &str,
    secret: &str,
) -> Result<(), String> {
    if secret.is_empty() {
        store.delete_secret(connection_id, key)
    } else {
        store.set_secret(connection_id, key, secret)
    }
}

fn persist_optional_secret(
    store: &dyn ConnectionSecretStore,
    connection_id: &str,
    key: &str,
    secret: Option<&str>,
) -> Result<(), String> {
    match secret.filter(|secret| !secret.is_empty()) {
        Some(secret) => store.set_secret(connection_id, key, secret),
        None => store.delete_secret(connection_id, key),
    }
}

fn read_connections(path: &Path) -> Result<Vec<ConnectionConfig>, String> {
    let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&json).map_err(|e| e.to_string())
}

fn write_sanitized_connections(path: &Path, configs: &[ConnectionConfig]) -> Result<(), String> {
    let sanitized = sanitize_connections(configs);
    let json = serde_json::to_string_pretty(&sanitized).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

fn sanitize_connections(configs: &[ConnectionConfig]) -> Vec<ConnectionConfig> {
    configs
        .iter()
        .cloned()
        .map(|mut config| {
            config.password.clear();
            config.ssh_password.clear();
            config.ssh_key_passphrase.clear();
            config.connection_string = None;
            clear_feishu_access_token(&mut config);
            config
        })
        .collect()
}

pub fn secret_account(connection_id: &str, key: &str) -> String {
    format!("connection:{connection_id}:{key}")
}

fn is_feishu_connection(db_type: &DatabaseType) -> bool {
    matches!(db_type, DatabaseType::FeishuSheets | DatabaseType::FeishuBitable)
}

fn feishu_access_token(config: &ConnectionConfig) -> Option<String> {
    if !is_feishu_connection(&config.db_type) {
        return None;
    }
    config
        .external_config
        .as_ref()
        .and_then(|value| value.get("access_token"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn clear_feishu_access_token(config: &mut ConnectionConfig) {
    if !is_feishu_connection(&config.db_type) {
        return;
    }
    if let Some(serde_json::Value::Object(object)) = config.external_config.as_mut() {
        object.remove("access_token");
    }
}

fn set_feishu_access_token(config: &mut ConnectionConfig, token: String) {
    if !is_feishu_connection(&config.db_type) {
        return;
    }
    let mut object = match config.external_config.take() {
        Some(serde_json::Value::Object(object)) => object,
        _ => serde_json::Map::new(),
    };
    object.insert("access_token".to_string(), serde_json::Value::String(token));
    config.external_config = Some(serde_json::Value::Object(object));
}

#[cfg(test)]
mod tests {
    use super::{
        feishu_access_token, load_connections_from_file, save_connections_to_file, ConnectionSecretStore,
        CONNECTION_STRING_KEY, FEISHU_ACCESS_TOKEN_KEY, MAIN_PASSWORD_KEY, SSH_PASSWORD_KEY,
    };
    use crate::models::connection::{ConnectionConfig, DatabaseType};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::Path;

    #[derive(Default)]
    struct MemorySecretStore {
        values: RefCell<HashMap<String, String>>,
        deleted: RefCell<Vec<String>>,
    }

    impl MemorySecretStore {
        fn set_existing(&self, connection_id: &str, key: &str, value: &str) {
            self.values.borrow_mut().insert(secret_key(connection_id, key), value.to_string());
        }

        fn get_existing(&self, connection_id: &str, key: &str) -> Option<String> {
            self.values.borrow().get(&secret_key(connection_id, key)).cloned()
        }

        fn was_deleted(&self, connection_id: &str, key: &str) -> bool {
            self.deleted.borrow().contains(&secret_key(connection_id, key))
        }
    }

    impl ConnectionSecretStore for MemorySecretStore {
        fn set_secret(&self, connection_id: &str, key: &str, secret: &str) -> Result<(), String> {
            self.values.borrow_mut().insert(secret_key(connection_id, key), secret.to_string());
            Ok(())
        }

        fn get_secret(&self, connection_id: &str, key: &str) -> Result<Option<String>, String> {
            Ok(self.values.borrow().get(&secret_key(connection_id, key)).cloned())
        }

        fn delete_secret(&self, connection_id: &str, key: &str) -> Result<(), String> {
            self.values.borrow_mut().remove(&secret_key(connection_id, key));
            self.deleted.borrow_mut().push(secret_key(connection_id, key));
            Ok(())
        }
    }

    fn secret_key(connection_id: &str, key: &str) -> String {
        format!("{connection_id}:{key}")
    }

    fn temp_connections_file(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dbx-connection-secrets-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("connections.json")
    }

    fn connection(id: &str, password: &str, ssh_password: &str) -> ConnectionConfig {
        ConnectionConfig {
            id: id.to_string(),
            name: format!("{id} connection"),
            db_type: DatabaseType::Postgres,
            driver_profile: None,
            driver_label: None,
            url_params: None,
            host: "localhost".to_string(),
            port: 5432,
            username: "postgres".to_string(),
            password: password.to_string(),
            database: Some("postgres".to_string()),
            color: None,
            ssh_enabled: !ssh_password.is_empty(),
            ssh_host: String::new(),
            ssh_port: 22,
            ssh_user: String::new(),
            ssh_password: ssh_password.to_string(),
            ssh_key_path: String::new(),
            ssh_key_passphrase: String::new(),
            ssh_expose_lan: false,
            ssh_connect_timeout_secs: crate::models::connection::default_ssh_connect_timeout_secs(),
            ssl: false,
            sysdba: false,
            connection_string: None,
            external_config: None,
        }
    }

    fn read_configs(path: &Path) -> Vec<ConnectionConfig> {
        let json = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    fn feishu_connection(id: &str, token: &str) -> ConnectionConfig {
        let mut config = connection(id, "app-secret", "");
        config.db_type = DatabaseType::FeishuBitable;
        config.driver_profile = Some("feishu_bitable".to_string());
        config.driver_label = Some("Feishu Bitable".to_string());
        config.host = "https://open.feishu.cn".to_string();
        config.port = 0;
        config.database = None;
        config.external_config = Some(serde_json::json!({
            "access_token": token,
            "app_token": "app_test"
        }));
        config
    }

    #[test]
    fn save_connections_moves_passwords_to_secret_store_and_redacts_file() {
        let path = temp_connections_file("save-redacts");
        let store = MemorySecretStore::default();
        let configs = vec![connection("main", "db-secret", "ssh-secret")];

        save_connections_to_file(&path, &configs, &store).unwrap();

        assert_eq!(store.get_existing("main", MAIN_PASSWORD_KEY).as_deref(), Some("db-secret"));
        assert_eq!(store.get_existing("main", SSH_PASSWORD_KEY).as_deref(), Some("ssh-secret"));
        let persisted = read_configs(&path);
        assert_eq!(persisted[0].password, "");
        assert_eq!(persisted[0].ssh_password, "");
    }

    #[test]
    fn load_connections_restores_passwords_from_secret_store() {
        let path = temp_connections_file("load-restores");
        let store = MemorySecretStore::default();
        store.set_existing("main", MAIN_PASSWORD_KEY, "db-secret");
        store.set_existing("main", SSH_PASSWORD_KEY, "ssh-secret");
        let sanitized = vec![connection("main", "", "")];
        std::fs::write(&path, serde_json::to_string_pretty(&sanitized).unwrap()).unwrap();

        let loaded = load_connections_from_file(&path, &store).unwrap();

        assert_eq!(loaded[0].password, "db-secret");
        assert_eq!(loaded[0].ssh_password, "ssh-secret");
    }

    #[test]
    fn load_connections_migrates_plaintext_passwords_and_rewrites_sanitized_file() {
        let path = temp_connections_file("migrates-plaintext");
        let store = MemorySecretStore::default();
        let legacy = vec![connection("legacy", "plain-db", "plain-ssh")];
        std::fs::write(&path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        let loaded = load_connections_from_file(&path, &store).unwrap();

        assert_eq!(loaded[0].password, "plain-db");
        assert_eq!(loaded[0].ssh_password, "plain-ssh");
        assert_eq!(store.get_existing("legacy", MAIN_PASSWORD_KEY).as_deref(), Some("plain-db"));
        assert_eq!(store.get_existing("legacy", SSH_PASSWORD_KEY).as_deref(), Some("plain-ssh"));
        let persisted = read_configs(&path);
        assert_eq!(persisted[0].password, "");
        assert_eq!(persisted[0].ssh_password, "");
    }

    #[test]
    fn save_connections_deletes_secrets_for_removed_connections() {
        let path = temp_connections_file("deletes-removed");
        let store = MemorySecretStore::default();
        let previous = vec![connection("old", "", ""), connection("kept", "", "")];
        std::fs::write(&path, serde_json::to_string_pretty(&previous).unwrap()).unwrap();
        store.set_existing("old", MAIN_PASSWORD_KEY, "old-db");
        store.set_existing("old", SSH_PASSWORD_KEY, "old-ssh");
        store.set_existing("kept", MAIN_PASSWORD_KEY, "kept-db");

        save_connections_to_file(&path, &[connection("kept", "new-db", "")], &store).unwrap();

        assert!(store.was_deleted("old", MAIN_PASSWORD_KEY));
        assert!(store.was_deleted("old", SSH_PASSWORD_KEY));
        assert_eq!(store.get_existing("kept", MAIN_PASSWORD_KEY).as_deref(), Some("new-db"));
    }

    #[test]
    fn save_connections_moves_connection_string_to_secret_store_and_restores_it() {
        let path = temp_connections_file("connection-string");
        let store = MemorySecretStore::default();
        let mut config = connection("mongo", "", "");
        config.db_type = DatabaseType::MongoDb;
        config.connection_string = Some("mongodb://user:secret@localhost/app".to_string());

        save_connections_to_file(&path, &[config], &store).unwrap();

        assert_eq!(
            store.get_existing("mongo", CONNECTION_STRING_KEY).as_deref(),
            Some("mongodb://user:secret@localhost/app")
        );
        let persisted = read_configs(&path);
        assert_eq!(persisted[0].connection_string, None);

        let loaded = load_connections_from_file(&path, &store).unwrap();
        assert_eq!(loaded[0].connection_string.as_deref(), Some("mongodb://user:secret@localhost/app"));
    }

    #[test]
    fn save_connections_moves_feishu_access_token_to_secret_store_and_restores_it() {
        let path = temp_connections_file("feishu-token");
        let store = MemorySecretStore::default();
        let config = feishu_connection("feishu", "tenant-token");

        save_connections_to_file(&path, &[config], &store).unwrap();

        assert_eq!(store.get_existing("feishu", FEISHU_ACCESS_TOKEN_KEY).as_deref(), Some("tenant-token"));
        let persisted = read_configs(&path);
        let persisted_json = serde_json::to_string(&persisted[0].external_config).unwrap();
        assert!(!persisted_json.contains("tenant-token"));
        assert!(!persisted_json.contains("access_token"));

        let loaded = load_connections_from_file(&path, &store).unwrap();
        assert_eq!(feishu_access_token(&loaded[0]).as_deref(), Some("tenant-token"));
    }

    #[test]
    fn load_connections_migrates_legacy_feishu_access_token_and_rewrites_sanitized_file() {
        let path = temp_connections_file("legacy-feishu-token");
        let store = MemorySecretStore::default();
        let legacy = vec![feishu_connection("legacy", "legacy-token")];
        std::fs::write(&path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        let loaded = load_connections_from_file(&path, &store).unwrap();

        assert_eq!(feishu_access_token(&loaded[0]).as_deref(), Some("legacy-token"));
        assert_eq!(store.get_existing("legacy", FEISHU_ACCESS_TOKEN_KEY).as_deref(), Some("legacy-token"));
        let persisted = read_configs(&path);
        let persisted_json = serde_json::to_string(&persisted[0].external_config).unwrap();
        assert!(!persisted_json.contains("legacy-token"));
        assert!(!persisted_json.contains("access_token"));
    }
}
