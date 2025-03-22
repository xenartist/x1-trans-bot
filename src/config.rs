use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use anyhow::{Result, Context, anyhow};
use solana_sdk::{
    signature::{Keypair, Signer},
    pubkey::Pubkey,
    signer::keypair::keypair_from_seed,
};
use std::str::FromStr;
use bip39::{Mnemonic, Language};
use rand::{rngs::OsRng, RngCore};
use solana_sdk::signature::read_keypair_file;
use std::error::Error;
use crate::wallet::WalletManager;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    // RPC endpoint URLs list
    pub rpc_urls: Vec<String>,
    // Seed passphrase for HD wallet generation (BIP39 mnemonic)
    pub seed_passphrase: Option<String>,
    // Master public key for funding derived wallets
    pub master_pubkey: Option<String>,
    // Target public key for transfers
    pub target_pubkey: String,
    // Number of concurrent transactions
    pub concurrency: usize,
    // Amount to transfer each time (in SOL)
    pub amount: f64,
}

impl Default for Config {
    fn default() -> Self {
        // generate new 12 words seed phrase
        let seed_phrase = generate_mnemonic_12_words()
            .expect("Failed to generate seed words");
        
        // generate master keypair from seed phrase
        let seed = seed_to_bytes(&seed_phrase)
            .expect("Failed to create seed from mnemonic");
            
        let master_keypair = keypair_from_seed(&seed[..32])
            .expect("Failed to create keypair from seed");
        
        let master_pubkey = master_keypair.pubkey().to_string();

        Self {
            rpc_urls: vec![
                "https://rpc.testnet.x1.xyz".to_string(),
            ],
            seed_passphrase: Some(seed_phrase),
            master_pubkey: Some(master_pubkey),
            target_pubkey: "B6eiBpErfZAsMN29N1ug9r9cLnd9NBVGA4rw3ABHXA1J".to_string(),
            concurrency: 8,
            amount: 0.000001,
        }
    }
}

impl Config {
    pub fn load(config_path: &str) -> Result<Self> {
        if let Ok(content) = fs::read_to_string(config_path) {
            let config: Config = serde_json::from_str(&content)
                .context("Failed to parse config file")?;
            Ok(config)
        } else {
            let config = Config::default();
            Ok(config)
        }
    }

    pub fn save(&self, config_path: &str) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        
        // Create parent directories if they don't exist
        if let Some(parent) = PathBuf::from(config_path).parent() {
            fs::create_dir_all(parent)?;
        }
        
        fs::write(config_path, content)?;
        Ok(())
    }
    
    // Get master keypair from seed passphrase
    pub fn get_master_keypair(&self) -> Result<Keypair> {
        match &self.seed_passphrase {
            Some(seed_phrase) => {
                // generate seed from seed phrase
                let seed = seed_to_bytes(seed_phrase)
                    .map_err(|e| anyhow!("Failed to create seed: {}", e))?;
                
                // create keypair from seed
                keypair_from_seed(&seed[..32])
                    .map_err(|e| anyhow!("Failed to create keypair from seed: {}", e))
            },
            None => Err(anyhow!("No seed passphrase provided in config"))
        }
    }
    
    // Get master pubkey
    pub fn get_master_pubkey(&self) -> Result<Pubkey> {
        match &self.master_pubkey {
            Some(pubkey_str) => {
                Pubkey::from_str(pubkey_str)
                    .context("Invalid master pubkey format")
            },
            None => {
                // Try to derive from seed
                let keypair = self.get_master_keypair()?;
                Ok(keypair.pubkey())
            }
        }
    }
}

// generate 12 words seed phrase
fn generate_mnemonic_12_words() -> Result<String, String> {
    // use correct method to get seed phrase
    let mut entropy = [0u8; 16]; // 16 bytes 对应 12 个词
    // use correct method to fill entropy
    OsRng.fill_bytes(&mut entropy);
    
    let mnemonic = Mnemonic::from_entropy(&entropy)
        .map_err(|e| format!("Failed to generate mnemonic: {}", e))?;
        
    // use correct method to get seed phrase
    Ok(mnemonic.to_string())
}

// generate seed from seed phrase
fn seed_to_bytes(seed_phrase: &str) -> Result<Vec<u8>, String> {
    // use BIP39 to convert seed phrase to seed
    // fix method call
    let mnemonic = Mnemonic::parse_normalized(seed_phrase)
        .map_err(|e| format!("Invalid mnemonic: {}", e))?;
        
    // use empty password to generate seed
    let seed = mnemonic.to_seed("");
    
    Ok(seed.to_vec())
} 