//! What a request used: the tokens a service counted for it, and for
//! DeepSeek, whose API says how much of a request it read from its cache
//! and what the account still holds, about what that cost.
//!
//! The counts are the service's own (`ai::TurnReader`). The cost is an
//! estimate from the prices DeepSeek lists; the balance is what its
//! `/user/balance` answers. The HTTP is in `wisp_net`.

use crate::ai::{HttpRequest, Profile};
use serde_json::Value;

/// The tokens of one request, as the service counted them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// The tokens of the request, those read from the service's cache among them.
    pub input: usize,
    /// How many of them were read from its cache; `None` where the service does not say.
    pub cached: Option<usize>,
    /// The tokens of the answer, its thinking among them.
    pub output: usize,
    /// How many of those were thinking, where the service says.
    pub reasoning: usize,
}

impl Usage {
    /// Count another request of the same question in.
    pub fn add(&mut self, more: &Usage) {
        self.input += more.input;
        self.output += more.output;
        self.reasoning += more.reasoning;
        if let Some(cached) = more.cached { self.cached = Some(self.cached.unwrap_or(0) + cached) }
    }

    /// The tokens of the request the service did not have in its cache.
    pub fn missed(&self) -> usize {
        self.input.saturating_sub(self.cached.unwrap_or(0))
    }
}

/// Whether a profile's service is DeepSeek's: its own address, or a provider set up from its preset (which
/// keeps the preset's key variable when its address is changed to a mirror). Where the cache counts, the
/// prices and the balance below hold.
pub fn is_deepseek(profile: &Profile) -> bool {
    profile.key_env == "DEEPSEEK_API_KEY" || crate::provider::preset_of_address(&profile.base_url).is_some_and(|preset| preset.key == "deepseek")
}

/// DeepSeek's prices in US dollars per million tokens at peak hours: input
/// read from its cache, input that was not, output. Off-peak is half. As
/// listed at api-docs.deepseek.com/quick_start/pricing on 2026-11-22; a
/// model not listed here gets no estimate.
const DEEPSEEK_PRICES: [(&str, [f64; 3]); 3] = [("deepseek-flash", [0.006, 0.3, 1.2]), ("deepseek-v4-flash", [0.006, 0.3, 1.2]), ("deepseek-v4-pro", [0.044, 1.32, 3.96])];

/// Whether DeepSeek charges its peak price at this time (seconds since
/// 1970): 01:00 to 04:00 and 06:00 to 10:00 UTC, Monday to Friday. Chinese
/// public holidays are off-peak too, which is not known here: such a day
/// is counted as any weekday, so the estimate errs on the high side.
pub fn deepseek_peak(unix_seconds: i64) -> bool {
    let (day, second) = (unix_seconds.div_euclid(86_400), unix_seconds.rem_euclid(86_400));
    // 1970-01-01 was a Thursday.
    let weekday = (day + 3).rem_euclid(7);
    let hour = second / 3_600;
    weekday < 5 && ((1..4).contains(&hour) || (6..10).contains(&hour))
}

/// About what a request to DeepSeek cost, in US dollars, at the prices
/// listed for its model at that time. `None` for a model without a listed price.
pub fn deepseek_cost(model: &str, usage: &Usage, unix_seconds: i64) -> Option<f64> {
    let (_, [hit, miss, output]) = DEEPSEEK_PRICES.iter().find(|(name, _)| *name == model.trim())?;
    let rate = if deepseek_peak(unix_seconds) { 1. } else { 0.5 };
    Some((usage.cached.unwrap_or(0) as f64 * hit + usage.missed() as f64 * miss + usage.output as f64 * output) * rate / 1e6)
}

/// The request for what a DeepSeek account still holds: `GET /user/balance` at the service's host, with the profile's key.
pub fn balance_request(profile: &Profile) -> Result<HttpRequest, String> {
    let key = profile.key();
    if key.is_empty() { return Err("还没有配置 API Key。".into()) }
    let base = profile.base_url.trim();
    let (scheme, rest) = base.split_once("://").ok_or("模型服务地址应以 https:// 或 http:// 开头。")?;
    let host = rest.split('/').next().unwrap_or(rest);
    Ok(HttpRequest { url: format!("{scheme}://{host}/user/balance"), headers: vec![("authorization".into(), format!("Bearer {key}")), ("accept".into(), "application/json".into())], body: Value::Null, proxy: profile.proxy.trim().to_string() })
}

/// What an account holds, as the service words it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Balance {
    /// Enough is left for a request.
    pub available: bool,
    /// The total in each currency the account has: (`CNY` or `USD`, the amount as written, `110.00`).
    pub amounts: Vec<(String, String)>,
}

impl Balance {
    /// The amounts as they are shown: `¥110.00`, two currencies side by side.
    pub fn text(&self) -> String {
        self.amounts.iter().map(|(currency, amount)| format!("{}{amount}", match currency.as_str() { "CNY" => "¥", "USD" => "$", other => other })).collect::<Vec<_>>().join(" / ")
    }
}

/// Read the answer of `/user/balance`.
pub fn read_balance(body: &str) -> Result<Balance, String> {
    let value: Value = serde_json::from_str(body).map_err(|_| "余额接口的回答无法读取。".to_string())?;
    let amounts: Vec<(String, String)> = value["balance_infos"].as_array().into_iter().flatten().filter_map(|info| Some((info["currency"].as_str()?.to_string(), info["total_balance"].as_str()?.to_string()))).collect();
    if amounts.is_empty() { return Err("余额接口没有给出余额。".into()) }
    Ok(Balance { available: value["is_available"].as_bool().unwrap_or(true), amounts })
}

/// A count of tokens as it is shown: `856`, `12.3K`, `1.2M`.
pub fn count(tokens: usize) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{:.1}K", tokens as f64 / 1e3),
        _ => format!("{:.2}M", tokens as f64 / 1e6),
    }
}

/// An estimated cost as it is shown: four places, since a question is often a fraction of a cent.
pub fn money(dollars: f64) -> String {
    if dollars > 0. && dollars < 0.0001 { "<$0.0001".into() } else { format!("${dollars:.4}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Protocol;

    #[test]
    fn requests_add_up() {
        let mut all = Usage::default();
        all.add(&Usage { input: 1000, cached: None, output: 50, reasoning: 0 });
        assert_eq!((all.cached, all.missed()), (None, 1000));
        all.add(&Usage { input: 3000, cached: Some(2400), output: 200, reasoning: 120 });
        assert_eq!(all, Usage { input: 4000, cached: Some(2400), output: 250, reasoning: 120 });
        assert_eq!(all.missed(), 1600);
        assert_eq!([count(856), count(12_345), count(1_234_567)], ["856", "12.3K", "1.23M"]);
        assert_eq!([money(0.), money(0.00004), money(0.01234)], ["$0.0000", "<$0.0001", "$0.0123"]);
    }

    #[test]
    fn deepseek_prices_and_hours() {
        // 2026-11-23 is a Monday.
        let monday: i64 = 1_795_392_000;
        assert_eq!(monday.rem_euclid(86_400), 0);
        let at = |hour: i64| monday + hour * 3_600;
        assert_eq!([0, 1, 3, 4, 5, 6, 9, 10, 23].map(|hour| deepseek_peak(at(hour))), [false, true, true, false, false, true, true, false, false]);
        // Friday is a weekday; Saturday and Sunday are off-peak all day.
        assert_eq!([4, 5, 6].map(|day| deepseek_peak(at(day * 24 + 2))), [true, false, false]);
        let usage = Usage { input: 1_000_000, cached: Some(800_000), output: 100_000, reasoning: 0 };
        let peak = deepseek_cost("deepseek-flash", &usage, at(2)).unwrap();
        assert!((peak - (0.8 * 0.006 + 0.2 * 0.3 + 0.1 * 1.2)).abs() < 1e-9, "{peak}");
        assert!((deepseek_cost("deepseek-flash", &usage, at(12)).unwrap() - peak / 2.).abs() < 1e-9);
        assert!(deepseek_cost("deepseek-v4-pro", &usage, at(2)).unwrap() > 3. * peak);
        assert_eq!(deepseek_cost("deepseek-next", &usage, at(2)), None);
    }

    #[test]
    fn deepseek_balance() {
        let profile = |base: &str| Profile { protocol: Protocol::OpenAI, base_url: base.into(), api_key: "k".into(), proxy: " direct ".into(), ..Default::default() };
        assert!(is_deepseek(&profile("https://api.deepseek.com")) && is_deepseek(&profile("https://api.deepseek.com/v1/")));
        assert!(!is_deepseek(&profile("https://api.moonshot.cn/v1")) && !is_deepseek(&profile("http://127.0.0.1:3425/v1")));
        assert!(is_deepseek(&Profile { key_env: "DEEPSEEK_API_KEY".into(), ..profile("http://127.0.0.1:8790") }));
        let request = balance_request(&profile("https://api.deepseek.com/v1/")).unwrap();
        assert_eq!((request.url.as_str(), request.proxy.as_str()), ("https://api.deepseek.com/user/balance", "direct"));
        assert_eq!(request.headers[0], ("authorization".to_string(), "Bearer k".to_string()));
        assert!(balance_request(&Profile { api_key: String::new(), key_env: "WISP_TEST_NO_SUCH_KEY".into(), ..profile("https://api.deepseek.com") }).is_err());

        let balance = read_balance(r#"{"is_available": true, "balance_infos": [{"currency": "CNY", "total_balance": "110.00", "granted_balance": "10.00", "topped_up_balance": "100.00"}]}"#).unwrap();
        assert_eq!((balance.available, balance.text().as_str()), (true, "¥110.00"));
        let two = read_balance(r#"{"is_available": false, "balance_infos": [{"currency": "CNY", "total_balance": "0.00"}, {"currency": "USD", "total_balance": "1.50"}]}"#).unwrap();
        assert_eq!((two.available, two.text().as_str()), (false, "¥0.00 / $1.50"));
        assert!(read_balance(r#"{"error": {"message": "Authentication Fails"}}"#).is_err() && read_balance("<html>").is_err());
    }
}
