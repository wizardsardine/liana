use std::time::Duration;

/// Follow a redirect chain to get the final URL
pub async fn follow_redirect_chain(initial_url: String) -> Result<String, String> {
    tracing::info!("Following redirect chain for URL: {}", initial_url);
    
    // Create HTTP client with redirect following disabled so we can handle it manually
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let mut current_url = initial_url;
    let mut redirect_count = 0;
    const MAX_REDIRECTS: usize = 10;

    loop {
        if redirect_count >= MAX_REDIRECTS {
            return Err("Too many redirects".to_string());
        }

        tracing::info!("Requesting URL: {}", current_url);
        
        let response = client
            .get(&current_url)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        let status = response.status();
        tracing::info!("Response status: {}", status);

        if status.is_redirection() {
            // Handle redirect
            if let Some(location) = response.headers().get("location") {
                let location_str = location
                    .to_str()
                    .map_err(|e| format!("Invalid location header: {}", e))?;
                
                // Handle relative URLs
                current_url = if location_str.starts_with("http") {
                    location_str.to_string()
                } else {
                    // Resolve relative URL
                    let base_url = url::Url::parse(&current_url)
                        .map_err(|e| format!("Invalid base URL: {}", e))?;
                    base_url
                        .join(location_str)
                        .map_err(|e| format!("Failed to resolve relative URL: {}", e))?
                        .to_string()
                };
                
                tracing::info!("Redirecting to: {}", current_url);
                redirect_count += 1;
                continue;
            } else {
                return Err("Redirect response without location header".to_string());
            }
        } else if status.is_success() {
            // Check if the response contains JavaScript redirects
            let body = response
                .text()
                .await
                .map_err(|e| format!("Failed to read response body: {}", e))?;

            tracing::info!("Checking HTML body for JavaScript redirects (length: {})", body.len());

            if let Some(js_redirect_url) = extract_javascript_redirect(&body) {
                tracing::info!("Found JavaScript redirect to: {}", js_redirect_url);

                // Handle relative URLs for JavaScript redirects too
                current_url = if js_redirect_url.starts_with("http") {
                    js_redirect_url
                } else {
                    // Resolve relative URL
                    let base_url = url::Url::parse(&current_url)
                        .map_err(|e| format!("Invalid base URL: {}", e))?;
                    base_url
                        .join(&js_redirect_url)
                        .map_err(|e| format!("Failed to resolve relative URL: {}", e))?
                        .to_string()
                };

                redirect_count += 1;
                continue;
            }

            // No more redirects, return the final URL
            tracing::info!("Final URL resolved: {}", current_url);
            return Ok(current_url);
        } else {
            return Err(format!("HTTP error: {}", status));
        }
    }
}

/// Extract JavaScript redirect URLs from HTML content
fn extract_javascript_redirect(html: &str) -> Option<String> {
    tracing::info!("Searching for JavaScript redirects in HTML content");

    // Look for common JavaScript redirect patterns
    let patterns = [
        // Standard location assignments
        r#"window\.location\.href\s*=\s*["']([^"']+)["']"#,
        r#"window\.location\s*=\s*["']([^"']+)["']"#,
        r#"location\.href\s*=\s*["']([^"']+)["']"#,
        r#"location\s*=\s*["']([^"']+)["']"#,
        r#"document\.location\s*=\s*["']([^"']+)["']"#,
        r#"document\.location\.href\s*=\s*["']([^"']+)["']"#,

        // Method calls
        r#"window\.location\.replace\s*\(\s*["']([^"']+)["']\s*\)"#,
        r#"window\.location\.assign\s*\(\s*["']([^"']+)["']\s*\)"#,
        r#"location\.replace\s*\(\s*["']([^"']+)["']\s*\)"#,
        r#"location\.assign\s*\(\s*["']([^"']+)["']\s*\)"#,

        // Meta refresh
        r#"<meta[^>]+http-equiv\s*=\s*["']refresh["'][^>]+content\s*=\s*["'][^;]*;\s*url\s*=\s*([^"']+)["']"#,

        // Common redirect patterns in Meld/payment providers
        r#"redirectUrl["']\s*:\s*["']([^"']+)["']"#,
        r#"redirect_url["']\s*:\s*["']([^"']+)["']"#,
        r#"targetUrl["']\s*:\s*["']([^"']+)["']"#,
        r#"target_url["']\s*:\s*["']([^"']+)["']"#,

        // Form action redirects
        r#"<form[^>]+action\s*=\s*["']([^"']*transak[^"']*)["']"#,
        r#"<form[^>]+action\s*=\s*["']([^"']*moonpay[^"']*)["']"#,

        // Iframe src redirects
        r#"<iframe[^>]+src\s*=\s*["']([^"']*transak[^"']*)["']"#,
        r#"<iframe[^>]+src\s*=\s*["']([^"']*moonpay[^"']*)["']"#,
    ];

    for (i, pattern) in patterns.iter().enumerate() {
        if let Ok(regex) = regex::Regex::new(pattern) {
            if let Some(captures) = regex.captures(html) {
                if let Some(url_match) = captures.get(1) {
                    let url = url_match.as_str().to_string();
                    tracing::info!("Found redirect URL with pattern {}: {}", i, url);
                    return Some(url);
                }
            }
        } else {
            tracing::warn!("Invalid regex pattern {}: {}", i, pattern);
        }
    }

    // Also look for any URLs containing known payment providers
    let provider_patterns = [
        r#"https?://[^"'\s]*transak[^"'\s]*"#,
        r#"https?://[^"'\s]*moonpay[^"'\s]*"#,
        r#"https?://[^"'\s]*simplex[^"'\s]*"#,
        r#"https?://[^"'\s]*banxa[^"'\s]*"#,
        r#"https?://[^"'\s]*ramp[^"'\s]*"#,
    ];

    for (i, pattern) in provider_patterns.iter().enumerate() {
        if let Ok(regex) = regex::Regex::new(pattern) {
            if let Some(url_match) = regex.find(html) {
                let url = url_match.as_str().to_string();
                tracing::info!("Found payment provider URL with pattern {}: {}", i, url);
                return Some(url);
            }
        }
    }

    tracing::info!("No JavaScript redirects found in HTML content");
    None
}

/// Check if a URL can be embedded in an iframe/webview
pub async fn is_url_embeddable(url: &str) -> bool {
    tracing::info!("Checking if URL is embeddable: {}", url);
    
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build();

    let Ok(client) = client else {
        tracing::warn!("Failed to create HTTP client for embeddability check");
        return false;
    };

    match client.head(url).send().await {
        Ok(response) => {
            let headers = response.headers();
            
            // Check X-Frame-Options header
            if let Some(x_frame_options) = headers.get("x-frame-options") {
                if let Ok(value) = x_frame_options.to_str() {
                    let value_lower = value.to_lowercase();
                    if value_lower.contains("deny") || value_lower.contains("sameorigin") {
                        tracing::info!("URL not embeddable due to X-Frame-Options: {}", value);
                        return false;
                    }
                }
            }
            
            // Check Content-Security-Policy header
            if let Some(csp) = headers.get("content-security-policy") {
                if let Ok(value) = csp.to_str() {
                    if value.contains("frame-ancestors 'none'") || value.contains("frame-ancestors 'self'") {
                        tracing::info!("URL not embeddable due to CSP frame-ancestors: {}", value);
                        return false;
                    }
                }
            }
            
            tracing::info!("URL appears to be embeddable");
            true
        }
        Err(e) => {
            tracing::warn!("Failed to check embeddability for {}: {}", url, e);
            // Assume not embeddable if we can't check
            false
        }
    }
}

/// Check if a URL is from a known payment provider that typically blocks embedding
pub fn is_payment_provider_url(url: &str) -> bool {
    let payment_providers = [
        "transak.com",
        "moonpay.com", 
        "simplex.com",
        "banxa.com",
        "ramp.network",
        "mercuryo.io",
        "coinify.com",
        "wyre.com",
    ];
    
    payment_providers.iter().any(|provider| url.contains(provider))
}

/// Get a user-friendly message for payment provider restrictions
pub fn get_payment_provider_message(url: &str) -> String {
    if url.contains("transak.com") {
        "Transak requires opening in a full browser for security reasons. Click 'Open in Browser' to continue with your purchase.".to_string()
    } else if url.contains("moonpay.com") {
        "MoonPay requires opening in a full browser for security reasons. Click 'Open in Browser' to continue with your purchase.".to_string()
    } else if is_payment_provider_url(url) {
        "This payment provider requires opening in a full browser for security reasons. Click 'Open in Browser' to continue with your purchase.".to_string()
    } else {
        "This service cannot be embedded and must be opened in a browser. Click 'Open in Browser' to continue.".to_string()
    }
}
