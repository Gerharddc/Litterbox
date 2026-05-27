use anyhow::{Context, Result, anyhow, bail};
use argon2::Argon2;
use inquire::{MultiSelect, Password};
use log::debug;
use russh::keys::{
    Algorithm, PrivateKey, decode_secret_key,
    pkcs8::{decode_pkcs8, encode_pkcs8_encrypted},
    ssh_key::LineEnding,
};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
};
use tabled::{Table, Tabled};

use crate::{
    agent::{AgentState, start_ssh_agent},
    files,
};

fn generate_private_key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("Ed25519 should be supported.")
}

fn key_to_openssh(key: &PrivateKey) -> Result<String> {
    Ok(key.to_openssh(LineEnding::LF)?.to_string())
}

fn hash_password(password: &str) -> String {
    use argon2::password_hash::PasswordHasher;

    Argon2::default()
        .hash_password(password.as_bytes())
        .expect("Passwords should be hashable")
        .to_string()
}

fn check_password(password: &str, hash: &str) -> bool {
    use argon2::password_hash::{PasswordVerifier, phc::PasswordHash};

    let parsed_hash = PasswordHash::new(hash).expect("Passwords should have valid hashes");

    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

#[derive(Debug, Deserialize, Serialize)]
struct Key {
    name: String,
    encrypted_key: Vec<u8>,
    attached_litterboxes: Vec<String>,
}

impl Key {
    fn new(name: &str, password: &str, private_key: &PrivateKey) -> Self {
        Self {
            name: name.to_owned(),
            encrypted_key: Self::encrypt(private_key, password),
            attached_litterboxes: Vec::new(),
        }
    }

    fn encrypt(private_key: &PrivateKey, password: &str) -> Vec<u8> {
        encode_pkcs8_encrypted(password.as_bytes(), 10, private_key)
            .expect("Keys should be encryptable")
    }

    fn decrypt(&self, password: &str) -> PrivateKey {
        decode_pkcs8(&self.encrypted_key, Some(password.as_bytes()))
            .expect("Key should have been encrypted with user password")
    }

    fn change_password(&mut self, old_password: &str, new_password: &str) {
        let decrypted = self.decrypt(old_password);

        self.encrypted_key = Self::encrypt(&decrypted, new_password);
    }
}

#[derive(Tabled)]
struct KeyTableRow {
    name: String,
    attached_litterboxes: String,
}

impl From<&Key> for KeyTableRow {
    fn from(value: &Key) -> Self {
        Self {
            name: value.name.clone(),
            attached_litterboxes: value.attached_litterboxes.join(","),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Keys {
    #[serde(default)]
    version: u32,
    password_hash: String,
    keys: Vec<Key>,
}

impl Keys {
    // TODO: perhaps we should place a lock on the keyfile while this struct exists?

    fn save_to_file(&self) -> Result<()> {
        let path = files::keyfile_path()?;
        let contents = ron::ser::to_string(self).context("failed to serialise keys")?;
        files::write_file(&path, &contents)
    }

    pub fn init_default() -> Result<Self> {
        eprintln!("Please enter a password to encrypt your keys.");
        let password = Password::new("Password:")
            .with_display_mode(inquire::PasswordDisplayMode::Masked)
            .prompt()?;
        let s = Self {
            version: 2,
            password_hash: hash_password(&password),
            keys: Vec::new(),
        };

        s.save_to_file()?;
        Ok(s)
    }

    pub fn load() -> Result<Self> {
        let keyfile = files::keyfile_path()?;
        if !keyfile.exists() {
            eprintln!("Keys file does not exist yet. A new one will be created.");
            return Self::init_default();
        }

        let contents = files::read_file(keyfile.as_path())?;
        let keys: Self = ron::from_str(&contents)?;

        if keys.version < 2 {
            bail!(
                "Your key file ({}) uses an older format that is incompatible with this version.\n\
                 A breaking change in the SSH key encryption library prevents decrypting existing keys.\n\
                 \n\
                 To migrate your keys:\n\
                 1. Downgrade Litterbox to version 0.4.2\n\
                 2. Make a backup: `cp {} {}.bak`\n\
                 3. Export all your keys: `litterbox keys export <name> <file>`\n\
                 4. Delete the keys file: `rm {}`\n\
                 5. Update to the latest version of Litterbox\n\
                 6. Run Litterbox again (a new keys file will be created)\n\
                 7. Import all your keys: `litterbox keys import <name> <file>`\n\
                 8. Delete the exported key files since they are not encrypted",
                keyfile.display(),
                keyfile.display(),
                keyfile.display(),
                keyfile.display(),
            );
        }

        Ok(keys)
    }

    pub fn print_list(&self) {
        let table_rows: Vec<KeyTableRow> = self.keys.iter().map(|c| c.into()).collect();
        let table = Table::new(table_rows);

        println!("{table}");
    }

    pub fn change_password(&mut self) -> Result<()> {
        let old_password = self.prompt_password()?;
        let new_password = Password::new("New password:")
            .with_display_mode(inquire::PasswordDisplayMode::Masked)
            .prompt()?;

        for key in &mut self.keys {
            key.change_password(&old_password, &new_password);
        }

        self.password_hash = hash_password(&new_password);
        self.save_to_file()?;
        Ok(())
    }

    fn prompt_password(&self) -> Result<String> {
        eprintln!("Please enter the password you chose to encrypt your keys.");

        loop {
            let password = Password::new("Password:")
                .with_display_mode(inquire::PasswordDisplayMode::Masked)
                .without_confirmation()
                .prompt()?;

            if check_password(&password, &self.password_hash) {
                return Ok(password);
            } else {
                eprintln!("The provided password is not correct. Please try again.");
            }
        }
    }

    fn key(&self, key_name: &str) -> Option<&Key> {
        self.keys.iter().find(|key| key.name == key_name)
    }

    fn key_mut(&mut self, key_name: &str) -> Option<&mut Key> {
        self.keys.iter_mut().find(|key| key.name == key_name)
    }

    pub fn generate(&mut self, key_name: &str) -> Result<()> {
        if self.key_mut(key_name).is_some() {
            bail!("Key \"{key_name}\" already exists.");
        }

        self.add(key_name, &generate_private_key())
    }

    pub fn add(&mut self, key_name: &str, private_key: &PrivateKey) -> Result<()> {
        let password = self.prompt_password()?;
        let key = Key::new(key_name, &password, private_key);

        self.keys.push(key);
        self.save_to_file()
    }

    pub fn delete(&mut self, key_name: &str) -> Result<()> {
        let mut found = false;

        self.keys.retain(|k| {
            if k.name == key_name {
                found = true;
                false
            } else {
                true
            }
        });

        if !found {
            bail!("Key \"{key_name}\" does not exist");
        }

        self.save_to_file()?;
        eprintln!("Deleted key \"{key_name}\"");
        Ok(())
    }

    pub fn attach(&mut self, key_name: &str, litterbox_name: &str) -> Result<()> {
        match self.key_mut(key_name) {
            Some(key) => {
                if key
                    .attached_litterboxes
                    .iter()
                    .any(|name| *name == litterbox_name)
                {
                    bail!(
                        "Key \"{key_name}\" is already attached to litterbox \"{litterbox_name}\""
                    );
                }

                key.attached_litterboxes.push(litterbox_name.to_owned());
                self.save_to_file()?;

                eprintln!("Attached \"{key_name}\" to litterbox \"{litterbox_name}\"!");
                Ok(())
            }

            None => bail!("Key \"{key_name}\" does not exist"),
        }
    }

    pub fn detach(&mut self, key_name: &str) -> Result<()> {
        match self.key_mut(key_name) {
            Some(key) => {
                let to_remove = MultiSelect::new(
                    "Select the litterboxes you want to detach:",
                    key.attached_litterboxes.clone(),
                )
                .prompt()?;

                key.attached_litterboxes
                    .retain(|name| !to_remove.contains(name));

                self.save_to_file()?;
                eprintln!(
                    "Detached {len} {lbox_word} from \"{key_name}\"!",
                    len = to_remove.len(),
                    lbox_word = if to_remove.len() == 1 {
                        "litterbox"
                    } else {
                        "litterboxes"
                    }
                );
                eprintln!("N.B. running litterboxes won't be affected until they are restarted!!");
                Ok(())
            }

            None => bail!("Key \"{key_name}\" does not exist"),
        }
    }

    fn attached_keys(&self, lbx_name: &str) -> Vec<&Key> {
        self.keys
            .iter()
            .filter(|key| key.attached_litterboxes.iter().any(|name| name == lbx_name))
            .collect()
    }

    fn has_attached_keys(&self, lbx_name: &str) -> bool {
        !self.attached_keys(lbx_name).is_empty()
    }

    pub fn password_if_needed(&self, lbx_name: &str) -> Result<Option<String>> {
        if self.has_attached_keys(lbx_name) {
            let password = self.prompt_password()?;
            Ok(Some(password))
        } else {
            Ok(None)
        }
    }

    pub async fn start_ssh_server(&self, lbx_name: &str, password: &str) -> Result<()> {
        let agent_state = Arc::new(AgentState::default());
        let agent_path = start_ssh_agent(lbx_name, agent_state.clone()).await?;
        debug!("agent_path: {:#?}", agent_path);

        let stream = tokio::net::UnixStream::connect(&agent_path)
            .await
            .context("Failed to connect to SSH agent socket")?;
        let mut client = russh::keys::agent::client::AgentClient::connect(stream);

        debug!("Registering keys to SSH agent.");
        for key in self.attached_keys(lbx_name) {
            log::info!("Registering key into agent: {}", key.name);

            let decrypted = key.decrypt(password);
            client
                .add_identity(&decrypted, &[])
                .await
                .context("Failed to register SSH key")?;
        }

        // Ensure the agent will now start prompting for authorization
        agent_state.locked.store(true, Ordering::SeqCst);

        Ok(())
    }

    pub fn print(&self, key_name: &str, private: bool) -> Result<()> {
        match self.key(key_name) {
            Some(key) => {
                let keys_password = self.prompt_password()?;
                let decrypted = key.decrypt(&keys_password);

                let output = if private {
                    key_to_openssh(&decrypted)?
                } else {
                    decrypted.public_key().to_openssh()?.to_string()
                };

                println!("{}", output);
                Ok(())
            }
            None => bail!("Key \"{key_name}\" does not exist"),
        }
    }

    pub fn export(&self, key_name: &str, path: &Path) -> Result<()> {
        // TODO: just let self.key return the correct error to begin with
        let key = self
            .key(key_name)
            .ok_or_else(|| anyhow!("Key \"{key_name}\" does not exist"))?;

        let keys_password = self.prompt_password()?;
        let decrypted = key.decrypt(&keys_password);
        let output = key_to_openssh(&decrypted)?;

        files::write_file(path, &output)?;
        eprintln!("Warning: The exported private key is unencrypted. Store it in a secure place!");
        eprintln!("Exported key \"{key_name}\" to {path:?}");

        Ok(())
    }

    pub fn import_key(&mut self, key_name: &str, file_path: PathBuf) -> Result<()> {
        if self.key(key_name).is_some() {
            bail!("Key \"{key_name}\" already exists. Please select a different name.");
        }

        let mut secret = String::new();
        std::fs::File::open(&file_path)
            .context("When opening file path")?
            .read_to_string(&mut secret)
            .context("When reading file")?;

        let mut password: Option<String> = None;
        let private_key = loop {
            use russh::keys::Error;
            use russh::keys::ssh_key::Error as SshKeyError;

            match decode_secret_key(&secret, password.as_deref()) {
                Ok(priv_key) => break priv_key,

                Err(Error::KeyIsEncrypted | Error::SshKey(SshKeyError::Crypto)) => {
                    if password.is_none() {
                        eprintln!("The key is encrypted. Please enter its password.");
                    } else {
                        eprintln!("The provided password is not correct. Please try again.");
                    };

                    password = Some(
                        Password::new("Password:")
                            .with_display_mode(inquire::PasswordDisplayMode::Masked)
                            .without_confirmation()
                            .prompt()?,
                    );
                }

                Err(cause) => bail!(cause),
            }
        };

        self.add(key_name, &private_key)?;
        eprintln!("Key \"{key_name}\" has been imported.");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_hash_and_verify_password() {
        let password = "some_random_pass";
        let hash = hash_password(password);
        assert_ne!(password, &hash);

        assert!(check_password(password, &hash));
        assert!(!check_password("wrong_pass", &hash));
    }

    #[test]
    fn can_encrypt_and_decrypt_password() {
        let password = "SomePassword";
        let original_key = generate_private_key();

        let encrypted_key = Key {
            name: String::new(),
            encrypted_key: Key::encrypt(&original_key, password),
            attached_litterboxes: Vec::new(),
        };
        let decrypted_key = encrypted_key.decrypt(password);
        assert_eq!(decrypted_key, original_key);
    }

    #[test]
    fn export_import_round_trip() {
        let key = generate_private_key();

        let exported = key_to_openssh(&key).unwrap();
        let imported = decode_secret_key(&exported, None).unwrap();

        assert_eq!(
            key.public_key().to_openssh().unwrap(),
            imported.public_key().to_openssh().unwrap()
        );
    }
}
