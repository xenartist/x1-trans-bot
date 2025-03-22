use anyhow::{anyhow, Result, Context};
use solana_client::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// RPC endpoint health status
#[derive(Debug, Clone)]
struct RpcEndpointHealth {
    /// endpoint URL
    url: String,
    /// last success call time
    last_success: Option<Instant>,
    /// last failure call time
    last_failure: Option<Instant>,
    /// consecutive failure count
    failure_count: usize,
    /// success rate (0.0-1.0)
    success_rate: f64,
    /// average response time (ms)
    avg_response_time: f64,
}

impl RpcEndpointHealth {
    fn new(url: String) -> Self {
        Self {
            url,
            last_success: None,
            last_failure: None,
            failure_count: 0,
            success_rate: 1.0, // initial assume fully healthy
            avg_response_time: 100.0, // initial assume 100ms
        }
    }

    /// record success call
    fn record_success(&mut self, response_time: Duration) {
        self.last_success = Some(Instant::now());
        self.failure_count = 0;
        
        // update success rate (70% old value, 30% new value)
        self.success_rate = self.success_rate * 0.7 + 0.3;
        
        // update average response time
        let response_ms = response_time.as_millis() as f64;
        self.avg_response_time = self.avg_response_time * 0.7 + response_ms * 0.3;
    }

    /// record failure call
    fn record_failure(&mut self) {
        self.last_failure = Some(Instant::now());
        self.failure_count += 1;
        
        // update success rate
        self.success_rate = self.success_rate * 0.7;
    }

    /// calculate health score (0-100)
    fn health_score(&self) -> f64 {
        // based on success rate, response time and consecutive failures
        let failure_penalty = match self.failure_count {
            0 => 1.0,
            1 => 0.9,
            2 => 0.7,
            3 => 0.5,
            _ => 0.1,
        };

        // response time score (1000ms below满分，5000ms above 0分)
        let response_score = (5000.0 - self.avg_response_time.min(5000.0)) / 5000.0;
        
        // combine scores
        (self.success_rate * 0.7 + response_score * 0.3) * failure_penalty * 100.0
    }

    /// check if endpoint should be temporarily disabled
    fn should_backoff(&self) -> bool {
        // disable after 5 consecutive failures
        self.failure_count >= 5
    }

    /// check if endpoint can be retried
    fn can_retry(&self) -> bool {
        if !self.should_backoff() {
            return true;
        }
        
        // if disabled for at least 30 seconds, allow retry
        if let Some(last_failure) = self.last_failure {
            return last_failure.elapsed() > Duration::from_secs(30);
        }
        
        true
    }
}

/// RPC client manager
#[derive(Clone)]
pub struct RpcManager {
    /// all RPC endpoints and their health status
    endpoints: Arc<Mutex<Vec<RpcEndpointHealth>>>,
    /// current polling index
    current_index: Arc<Mutex<usize>>,
}

impl RpcManager {
    /// create new RPC manager
    pub fn new(urls: Vec<String>) -> Self {
        let endpoints = urls.into_iter()
            .map(RpcEndpointHealth::new)
            .collect();
        
        Self {
            endpoints: Arc::new(Mutex::new(endpoints)),
            current_index: Arc::new(Mutex::new(0)),
        }
    }

    /// test all endpoints and return health status report
    pub async fn test_all_endpoints(&self) -> Result<String> {
        let mut report = String::new();
        let mut endpoints = self.endpoints.lock().unwrap();
        
        report.push_str("RPC Endpoints Health Report:\n");
        report.push_str("--------------------------\n");
        
        for endpoint in endpoints.iter_mut() {
            // create temporary client
            let client = RpcClient::new_with_commitment(
                endpoint.url.clone(),
                CommitmentConfig::confirmed(),
            );
            
            // test connection
            let start = Instant::now();
            let result = client.get_version();
            let elapsed = start.elapsed();
            
            match result {
                Ok(version) => {
                    endpoint.record_success(elapsed);
                    report.push_str(&format!(
                        "✅ {}: Score: {:.1}, Response: {}ms, Version: {:?}\n",
                        endpoint.url,
                        endpoint.health_score(),
                        elapsed.as_millis(),
                        version
                    ));
                },
                Err(e) => {
                    endpoint.record_failure();
                    report.push_str(&format!(
                        "❌ {}: Score: {:.1}, Error: {}\n",
                        endpoint.url,
                        endpoint.health_score(),
                        e
                    ));
                }
            }
        }
        
        // sort by health score
        endpoints.sort_by(|a, b| {
            b.health_score().partial_cmp(&a.health_score()).unwrap()
        });
        
        report.push_str("\nEndpoints sorted by health score (best first):\n");
        for (i, endpoint) in endpoints.iter().enumerate() {
            report.push_str(&format!(
                "{}. {} - Score: {:.1}\n",
                i + 1,
                endpoint.url,
                endpoint.health_score()
            ));
        }
        
        Ok(report)
    }

    /// get next RPC client
    pub fn next_client(&self) -> RpcClient {
        let mut endpoints = self.endpoints.lock().unwrap();
        let mut index = self.current_index.lock().unwrap();
        
        if endpoints.is_empty() {
            // if no endpoints, use default
            return RpcClient::new_with_commitment(
                "https://api.testnet.solana.com",
                CommitmentConfig::confirmed(),
            );
        }
        
        // try at most endpoints.len() times to find available endpoint
        for _ in 0..endpoints.len() {
            *index = (*index + 1) % endpoints.len();
            
            let endpoint = &mut endpoints[*index];
            
            // if endpoint is available, return client
            if endpoint.can_retry() {
                return RpcClient::new_with_commitment(
                    endpoint.url.clone(),
                    CommitmentConfig::confirmed(),
                );
            }
        }
        
        // if all endpoints are unavailable, use first one
        RpcClient::new_with_commitment(
            endpoints[0].url.clone(),
            CommitmentConfig::confirmed(),
        )
    }

    /// record client call result
    pub fn record_result(&self, url: &str, success: bool, response_time: Duration) {
        let mut endpoints = self.endpoints.lock().unwrap();
        
        if let Some(endpoint) = endpoints.iter_mut().find(|e| e.url == url) {
            if success {
                endpoint.record_success(response_time);
            } else {
                endpoint.record_failure();
            }
        }
    }
} 