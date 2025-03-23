use anyhow::{anyhow, Context, Result};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
    pubkey::Pubkey,
    hash::Hash,
};
use std::str::FromStr;
use std::sync::Arc;
use tokio::task;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering;
use std::time::Instant;
// use futures::future;

use crate::config::Config;
use crate::wallet::WalletManager;
use crate::rpc_manager::RpcManager;

pub struct TransferService {
    config: Config,
    wallet_manager: WalletManager,
    rpc_manager: RpcManager,
}

impl TransferService {
    pub fn new(config: Config) -> Result<Self> {
        // get master keypair
        let master_keypair = config.get_master_keypair()?;
        println!("Using master keypair for funding: {}", master_keypair.pubkey());
        
        // create wallet manager - pass master_keypair and seed passphrase
        let wallet_manager = WalletManager::new(
            master_keypair, 
            config.seed_passphrase.clone(),
            config.concurrency
        )?;
        
        // create rpc manager
        let rpc_manager = RpcManager::new(config.rpc_urls.clone());
        
        Ok(Self {
            config,
            wallet_manager,
            rpc_manager,
        })
    }
    
    /// test all rpc endpoints and output health report
    pub async fn test_rpc_endpoints(&self) -> Result<()> {
        let report = self.rpc_manager.test_all_endpoints().await?;
        println!("{}", report);
        Ok(())
    }

    pub async fn start_transfers(&self) -> Result<()> {
        // Test RPC endpoints first
        println!("Testing RPC endpoints...");
        self.test_rpc_endpoints().await?;
        
        // Calculate transfer amount (lamports)
        let lamports = (self.config.amount * 1_000_000_000.0) as u64;
        
        // Minimum balance threshold
        let min_balance_threshold = 100_000; // 0.0001 SOL in lamports
        
        // Fund derived wallets to 0.1 SOL
        println!("Funding derived wallets...");
        self.fund_derived_wallets(100_000_000).await?; // 0.1 SOL in lamports
        
        // Concurrency control
        let running_flag = Arc::new(AtomicBool::new(true));
        let success_count = Arc::new(AtomicUsize::new(0));
        let error_count = Arc::new(AtomicUsize::new(0));
        let total_tx_sent = Arc::new(AtomicUsize::new(0));
        let start_time = Instant::now();

        println!("Starting self-transfers (each wallet sends to itself)");
        println!("Amount per transfer: {} SOL ({} lamports)", 
            self.config.amount, 
            lamports
        );
        println!("Concurrency: {} (using {} derived wallets)", 
            self.config.concurrency,
            self.wallet_manager.derived_keypairs().len()
        );
        println!("Will stop when any wallet balance falls below: {} SOL", 
            min_balance_threshold as f64 / 1_000_000_000.0
        );
        println!("Using {} RPC endpoints", self.config.rpc_urls.len());

        // Collect derived wallet public keys for monitoring
        let derived_pubkeys: Vec<_> = self.wallet_manager.derived_keypairs()
            .iter()
            .map(|k| k.pubkey())
            .collect();
        
        // Start balance monitoring thread
        let running_flag_for_monitor = running_flag.clone();
        let rpc_manager = self.rpc_manager.clone();
        let total_tx_for_monitor = total_tx_sent.clone();
        let start_time_for_monitor = start_time;
        
        let monitor_handle = tokio::spawn(async move {
            let mut last_report = Instant::now();
            
            loop {
                // Get a client for checking balances
                let client = rpc_manager.next_client();
                let client_url = client.url().to_string();
                
                // Check all derived wallet balances
                for (i, pubkey) in derived_pubkeys.iter().enumerate() {
                    let start = Instant::now();
                    match client.get_balance(pubkey) {
                        Ok(balance) => {
                            rpc_manager.record_result(&client_url, true, start.elapsed());
                            
                            if balance < min_balance_threshold {
                                println!("Wallet {} balance is below threshold: {} SOL. Stopping all transfers.", 
                                    i, balance as f64 / 1_000_000_000.0);
                                running_flag_for_monitor.store(false, Ordering::SeqCst);
                                return;
                            }
                        },
                        Err(e) => {
                            rpc_manager.record_result(&client_url, false, start.elapsed());
                            println!("Failed to check wallet {} balance: {}", i, e);
                        }
                    }
                }
                
                // Show performance report every 10 seconds
                if last_report.elapsed().as_secs() >= 10 {
                    let elapsed = start_time_for_monitor.elapsed().as_secs();
                    let tx_count = total_tx_for_monitor.load(Ordering::SeqCst);
                    
                    let tps = if elapsed > 0 {
                        tx_count as f64 / elapsed as f64
                    } else {
                        0.0
                    };
                    
                    println!("Performance Report: Sent {} transactions in {}s ({:.2} TPS)", 
                        tx_count, elapsed, tps);
                    
                    last_report = Instant::now();
                }
                
                // Check balance every 5 seconds
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                
                // Exit monitor if stopped
                if !running_flag_for_monitor.load(Ordering::SeqCst) {
                    break;
                }
            }
        });

        // Start transfer tasks
        let mut handles = vec![];

        // Directly use derived_keypairs reference since Solana keypairs can't be cloned
        for (i, keypair) in self.wallet_manager.derived_keypairs().iter().enumerate() {
            // Get current wallet's pubkey
            let pubkey = keypair.pubkey();
            let success_counter = success_count.clone();
            let error_counter = error_count.clone();
            let is_running = running_flag.clone();
            let tx_counter = total_tx_sent.clone();
            let lamports = lamports;
            let rpc_manager = self.rpc_manager.clone();
            
            // Copy keypair bytes for task
            let keypair_bytes = keypair.to_bytes();
            
            let handle = tokio::spawn(async move {
                // Recreate keypair in task
                let task_keypair = Keypair::from_bytes(&keypair_bytes)
                    .expect("Failed to recreate keypair from bytes");
                
                println!("Task {} started with wallet {} (sending to self)", i, pubkey);
                
                // Pre-fetch blockhash occasionally to avoid fetching for every tx
                let mut recent_blockhash = rpc_manager.next_client().get_latest_blockhash()
                    .expect("Failed to get initial blockhash");
                let mut last_blockhash_update = Instant::now();
                
                while is_running.load(Ordering::SeqCst) {
                    // Get next RPC client
                    let client = rpc_manager.next_client();
                    let client_url = client.url().to_string();
                    
                    // Update blockhash every ~1 second instead of every transaction
                    if last_blockhash_update.elapsed().as_secs() >= 1 {
                        match client.get_latest_blockhash() {
                            Ok(blockhash) => {
                                recent_blockhash = blockhash;
                                last_blockhash_update = Instant::now();
                            },
                            Err(_) => {
                                // Silently continue using old blockhash if update fails
                                // This avoids interrupting the sending loop
                            }
                        }
                    }
                    
                    // Create transaction without waiting for confirmation
                    let instruction = system_instruction::transfer(
                        &task_keypair.pubkey(),
                        &pubkey,  // Send to self
                        lamports,
                    );
                    
                    let transaction = Transaction::new_signed_with_payer(
                        &[instruction],
                        Some(&task_keypair.pubkey()),
                        &[&task_keypair],
                        recent_blockhash,
                    );
                    
                    // Send transaction without waiting for confirmation or extensive error handling
                    match client.send_transaction(&transaction) {
                        Ok(signature) => {
                            // Minimal logging - consider removing for max performance
                            if success_counter.fetch_add(1, Ordering::SeqCst) % 100 == 0 {
                                println!("Task {}: Sent 100 transactions (latest: {})", i, signature);
                            }
                            tx_counter.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(e) => {
                            // Minimal error handling - only count errors
                            error_counter.fetch_add(1, Ordering::SeqCst);
                            
                            // Only stop on insufficient funds error
                            if e.to_string().contains("insufficient funds") {
                                println!("Task {}: Insufficient funds. Stopping.", i);
                                is_running.store(false, Ordering::SeqCst);
                                break;
                            }
                        }
                    }
                    
                    // Remove the sleep delay completely for maximum throughput
                    // If needed, you can add a yield_now() to prevent CPU monopolization
                    // tokio::task::yield_now().await;
                }
                
                println!("Task {} finished", i);
            });
            
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            let _ = handle.await;
        }
        
        // Ensure monitor thread also ends
        running_flag.store(false, Ordering::SeqCst);
        let _ = monitor_handle.await;

        let successful = success_count.load(Ordering::SeqCst);
        let failed = error_count.load(Ordering::SeqCst);
        let elapsed = start_time.elapsed().as_secs();
        let total = total_tx_sent.load(Ordering::SeqCst);
        
        println!("All transfers completed!");
        println!("Transactions sent: {}", successful);
        println!("Failed attempts: {}", failed);
        println!("Total elapsed time: {}s", elapsed);
        
        if elapsed > 0 {
            println!("Average TPS: {:.2}", total as f64 / elapsed as f64);
        }
        
        Ok(())
    }

    // Fund derived wallets to specified amount
    async fn fund_derived_wallets(&self, target_balance: u64) -> Result<()> {
        // Get a master RPC client
        let client = self.rpc_manager.next_client();
        
        // First check master wallet balance
        let master_balance = client.get_balance(&self.wallet_manager.master_keypair().pubkey())?;
        println!("Master wallet balance: {} SOL", master_balance as f64 / 1_000_000_000.0);
        
        let mut total_funding_needed = 0;
        let mut wallets_needing_funds = vec![];
        
        // First check all derived wallets, calculate total funding needed
        for (i, keypair) in self.wallet_manager.derived_keypairs().iter().enumerate() {
            let balance = client.get_balance(&keypair.pubkey())?;
            let balance_sol = balance as f64 / 1_000_000_000.0;
            
            if balance < target_balance {
                let amount_to_transfer = target_balance - balance;
                let amount_sol = amount_to_transfer as f64 / 1_000_000_000.0;
                
                println!("Wallet {} ({}) needs additional {} SOL (current: {} SOL)", 
                    i, 
                    keypair.pubkey().to_string(), 
                    amount_sol,
                    balance_sol
                );
                
                total_funding_needed += amount_to_transfer;
                wallets_needing_funds.push((i, keypair.pubkey(), amount_to_transfer));
            } else {
                println!("Wallet {} already has sufficient balance: {} SOL", 
                    i, 
                    balance_sol
                );
            }
        }
        
        // Check if master wallet has enough funds
        if total_funding_needed > 0 {
            if master_balance < total_funding_needed + 5_000_000 {  // Keep 0.005 SOL for fees
                return Err(anyhow::anyhow!("Master wallet has insufficient balance to fund derived wallets. Need {} SOL but only have {} SOL", 
                    (total_funding_needed + 5_000_000) as f64 / 1_000_000_000.0,
                    master_balance as f64 / 1_000_000_000.0
                ));
            }
            
            println!("Total funding needed: {} SOL", total_funding_needed as f64 / 1_000_000_000.0);
            
            // Send funds to each wallet that needs it
            for (i, pubkey, amount) in wallets_needing_funds {
                println!("Funding wallet {} ({}) with {} SOL", 
                    i, 
                    pubkey.to_string(), 
                    amount as f64 / 1_000_000_000.0
                );
                
                let instruction = system_instruction::transfer(
                    &self.wallet_manager.master_keypair().pubkey(),
                    &pubkey,
                    amount,
                );
                
                let recent_blockhash = client.get_latest_blockhash()?;
                
                let transaction = Transaction::new_signed_with_payer(
                    &[instruction],
                    Some(&self.wallet_manager.master_keypair().pubkey()),
                    &[self.wallet_manager.master_keypair()],
                    recent_blockhash,
                );
                
                let signature = client.send_and_confirm_transaction(&transaction)?;
                println!("  Funded with transaction: {}", signature);
            }
        } else {
            println!("All wallets are already funded to target balance");
        }
        
        Ok(())
    }
}

// batch transaction
fn create_batch_transaction(
    keypair: &Keypair,
    target: &Pubkey,
    lamports: u64,
    batch_size: usize,
    recent_blockhash: Hash
) -> Transaction {
    let mut instructions = Vec::with_capacity(batch_size);
    
    for _ in 0..batch_size {
        instructions.push(
            system_instruction::transfer(
                &keypair.pubkey(),
                target,
                lamports,
            )
        );
    }
    
    Transaction::new_signed_with_payer(
        &instructions,
        Some(&keypair.pubkey()),
        &[keypair],
        recent_blockhash,
    )
} 