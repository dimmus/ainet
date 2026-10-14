//! Turns transcript tokens into the figures to show: token totals first,
//! dollars as an estimate from a small price table.
//!
//! Ported from Den's `internal/cost` (MIT). A subscription pays no per-token
//! price, so a dollar figure here is what the same tokens would cost at API
//! list prices, never a bill. Prices are matched by the longest model-id
//! prefix; cache writes are priced at the five-minute cache rate (1.25 times
//! input). Claude Code's own figure (`cost-state` in the transcript, see
//! [`crate::transcript::Reported`]) is a cross-check, never summed.

use std::collections::{BTreeMap, HashMap};

use crate::transcript::{Account, Tokens};

/// When Den took the built-in table.
pub const PRICES_AS_OF: &str = "2026-09-25";

/// Dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

const fn p(input: f64, output: f64, cache_write: f64, cache_read: f64) -> Price {
    Price {
        input,
        output,
        cache_write,
        cache_read,
    }
}

/// One built-in rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    /// Model-id prefix.
    pub prefix: &'static str,
    pub price: Price,
    /// Not from Den's table: a stand-in until a real price is known.
    pub placeholder: bool,
}

/// Den's table (as of [`PRICES_AS_OF`]), plus placeholders for current
/// families Den does not price.
pub const BUILTIN: &[Rate] = &[
    Rate {
        prefix: "claude-fable-5",
        price: p(10.0, 50.0, 12.5, 1.0),
        placeholder: false,
    },
    Rate {
        prefix: "claude-fable-5-1",
        price: p(10.0, 50.0, 12.5, 0.25),
        placeholder: false,
    },
    Rate {
        prefix: "claude-mythos-5",
        price: p(10.0, 50.0, 12.5, 1.0),
        placeholder: false,
    },
    Rate {
        prefix: "claude-mythos-5-1",
        price: p(10.0, 50.0, 12.5, 0.25),
        placeholder: false,
    },
    Rate {
        prefix: "claude-opus-5-5",
        price: p(4.0, 20.0, 5.0, 0.20),
        placeholder: false,
    },
    Rate {
        prefix: "claude-opus-5",
        price: p(5.0, 25.0, 6.25, 0.50),
        placeholder: false,
    },
    Rate {
        prefix: "claude-opus-4",
        price: p(5.0, 25.0, 6.25, 0.50),
        placeholder: false,
    },
    Rate {
        prefix: "claude-sonnet-5",
        price: p(2.0, 10.0, 2.5, 0.20),
        placeholder: false,
    },
    Rate {
        prefix: "claude-sonnet-4",
        price: p(3.0, 15.0, 3.75, 0.30),
        placeholder: false,
    },
    Rate {
        prefix: "claude-haiku-4",
        price: p(1.0, 5.0, 1.25, 0.10),
        placeholder: false,
    },
    // TODO: Den has no Haiku 5 price. Placeholder at Haiku 4's rates until
    // the real list price is known; figures for it are marked partial.
    Rate {
        prefix: "claude-haiku-5",
        price: p(1.0, 5.0, 1.25, 0.10),
        placeholder: true,
    },
];

/// A price table: the built-in rates with overrides on top.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    rates: BTreeMap<String, (Price, bool)>,
}

impl Default for Table {
    fn default() -> Self {
        Table::new(&HashMap::new())
    }
}

impl Table {
    /// The built-in table with overrides applied (model-id prefix → price).
    /// An override value is "input, output, cache write, cache read" in
    /// dollars per million tokens; the last two may be left out and then
    /// follow the input rate (1.25 and 0.1 times). A value that does not
    /// parse is skipped. Den's `[prices]` config section.
    pub fn new(overrides: &HashMap<String, String>) -> Table {
        let mut rates: BTreeMap<String, (Price, bool)> = BUILTIN
            .iter()
            .map(|r| (r.prefix.to_string(), (r.price, r.placeholder)))
            .collect();
        for (k, v) in overrides {
            if let Some(price) = parse_price(v)
                && !k.is_empty()
            {
                rates.insert(k.clone(), (price, false));
            }
        }
        Table { rates }
    }

    fn lookup(&self, model: &str) -> Option<(Price, bool)> {
        self.rates
            .iter()
            .filter(|(k, _)| model.starts_with(k.as_str()))
            .max_by_key(|(k, _)| k.len())
            .map(|(_, v)| *v)
    }

    /// The price of a model by its longest matching prefix.
    pub fn price(&self, model: &str) -> Option<Price> {
        self.lookup(model).map(|(p, _)| p)
    }

    /// Whether the model's price is a placeholder.
    pub fn is_placeholder(&self, model: &str) -> bool {
        self.lookup(model).is_some_and(|(_, ph)| ph)
    }

    /// Prices tokens by model. `partial` is true when some model had no
    /// price (its tokens are left out) or only a placeholder.
    pub fn usd(&self, by_model: &HashMap<String, Tokens>) -> (f64, bool) {
        let mut usd = 0.0;
        let mut partial = false;
        for (model, tk) in by_model {
            if tk.total() == 0 {
                continue;
            }
            match self.lookup(model) {
                Some((p, placeholder)) => {
                    usd += cost(tk, &p);
                    partial |= placeholder;
                }
                None => partial = true,
            }
        }
        (usd, partial)
    }

    /// The summary of a session's account.
    pub fn of(&self, a: &Account) -> Summary {
        let (main, p1) = self.usd(&a.main);
        let (sub, p2) = self.usd(&a.sub);
        let mut s = Summary {
            tokens: sum(&a.main),
            subagents: sum(&a.sub),
            usd: main + sub,
            partial: p1 || p2,
            reported_usd: None,
        };
        if let Some(r) = a.reported {
            s.reported_usd = Some(r.usd);
            s.partial |= r.unknown;
        }
        s
    }

    /// The summary of tokens counted over a time window.
    pub fn of_window(&self, by_model: &HashMap<String, Tokens>) -> Summary {
        let (usd, partial) = self.usd(by_model);
        Summary {
            tokens: sum(by_model),
            usd,
            partial,
            ..Default::default()
        }
    }
}

fn cost(tk: &Tokens, p: &Price) -> f64 {
    (tk.input as f64 * p.input
        + tk.output as f64 * p.output
        + tk.cache_creation as f64 * p.cache_write
        + tk.cache_read as f64 * p.cache_read)
        / 1e6
}

fn sum(m: &HashMap<String, Tokens>) -> Tokens {
    m.values().fold(Tokens::default(), |a, t| a.add(t))
}

/// What `usage` of `model` costs at list price with the built-in table; 0
/// when the model has no price ([`estimate_checked`] tells).
pub fn estimate(usage: &Tokens, model: &str) -> f64 {
    estimate_checked(usage, model).unwrap_or(0.0)
}

/// [`estimate`], `None` when the model has no price.
pub fn estimate_checked(usage: &Tokens, model: &str) -> Option<f64> {
    let t = Table::default();
    t.price(model).map(|p| cost(usage, &p))
}

/// Reads "input, output[, cache write[, cache read]]".
pub fn parse_price(s: &str) -> Option<Price> {
    let mut n = Vec::new();
    for f in s.split([',', ' ']).filter(|f| !f.is_empty()) {
        let x: f64 = f.parse().ok()?;
        if x < 0.0 || !x.is_finite() {
            return None;
        }
        n.push(x);
    }
    if !(2..=4).contains(&n.len()) {
        return None;
    }
    Some(Price {
        input: n[0],
        output: n[1],
        cache_write: n.get(2).copied().unwrap_or(n[0] * 1.25),
        cache_read: n.get(3).copied().unwrap_or(n[0] * 0.1),
    })
}

/// What one agent, one team or one mission has used.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub tokens: Tokens,
    pub subagents: Tokens,
    /// The estimate.
    pub usd: f64,
    /// Some tokens had no (or only a placeholder) price.
    pub partial: bool,
    /// Claude Code's own figure; a cross-check, not summed.
    pub reported_usd: Option<f64>,
}

impl Summary {
    /// Every token of the agent and its subagents.
    pub fn total(&self) -> i64 {
        self.tokens.total() + self.subagents.total()
    }

    /// The sum of two summaries; the reported figure does not roll up.
    pub fn add(&self, o: &Summary) -> Summary {
        Summary {
            tokens: self.tokens.add(&o.tokens),
            subagents: self.subagents.add(&o.subagents),
            usd: self.usd + o.usd,
            partial: self.partial || o.partial,
            reported_usd: None,
        }
    }

    /// "12.4k tok (+3k subagents) · ≈ $0.42"
    pub fn line(&self) -> String {
        let mut out = format!("{} tok", tok(self.tokens.total()));
        if self.subagents.total() > 0 {
            out.push_str(&format!(" (+{} subagents)", tok(self.subagents.total())));
        }
        format!("{out} · {}", dollars(self.usd, self.partial))
    }
}

/// A token count, short: 950, 1.2k, 12k, 3.2M.
pub fn tok(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{}M", trim(&format!("{:.1}", n as f64 / 1e6)))
    } else if n >= 10_000 {
        format!("{}k", n / 1000)
    } else if n >= 1000 {
        format!("{}k", trim(&format!("{:.1}", n as f64 / 1e3)))
    } else {
        n.to_string()
    }
}

fn trim(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// An estimate: "≈ $0.42", with a "+" when part of it is unpriced.
pub fn dollars(usd: f64, partial: bool) -> String {
    let money = if usd >= 100.0 {
        format!("{usd:.0}")
    } else if usd >= 0.995 {
        format!("{usd:.2}")
    } else if usd == 0.0 {
        "0".into()
    } else {
        format!("{usd:.3}")
    };
    format!("≈ ${money}{}", if partial { "+" } else { "" })
}

/// A budget: tokens or estimated dollars.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Budget {
    Tokens(i64),
    Usd(f64),
}

/// Reads "2M", "500k", "1500000" as tokens and "$5" or "$0.50" as dollars.
pub fn parse_budget(s: &str) -> Result<Budget, String> {
    let s = s.trim().to_lowercase();
    if let Some(rest) = s.strip_prefix('$') {
        return match rest.parse::<f64>() {
            Ok(u) if u > 0.0 => Ok(Budget::Usd(u)),
            _ => Err(format!("budget {s:?}: use $5 for dollars")),
        };
    }
    let (num, mult) = if let Some(n) = s.strip_suffix('m') {
        (n, 1e6)
    } else if let Some(n) = s.strip_suffix('k') {
        (n, 1e3)
    } else {
        (s.as_str(), 1.0)
    };
    match num.parse::<f64>() {
        Ok(f) if f > 0.0 => Ok(Budget::Tokens((f * mult) as i64)),
        _ => Err("budget: use 2M or 500k for tokens, $5 for dollars".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Reported;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn longest_prefix() {
        let t = Table::default();
        for (model, input) in [
            ("claude-opus-5-5", 4.0),
            ("claude-opus-5", 5.0),
            ("claude-opus-4-8", 5.0),
            ("claude-sonnet-5-5", 2.0),
            ("claude-haiku-4-5-20251001", 1.0),
            ("claude-fable-5-1", 10.0),
            ("claude-opus-5-5[1m]", 4.0),
        ] {
            assert_eq!(t.price(model).map(|p| p.input), Some(input), "{model}");
        }
        assert_eq!(t.price("claude-fable-5-1").unwrap().cache_read, 0.25);
        assert!(t.price("gpt-x").is_none());
        assert!(t.is_placeholder("claude-haiku-5-1"));
        assert!(!t.is_placeholder("claude-haiku-4-5"));
    }

    #[test]
    fn usd_for_tokens() {
        let t = Table::new(&HashMap::from([(
            "claude-test".to_string(),
            "10, 50, 12.5, 1".to_string(),
        )]));
        let (usd, partial) = t.usd(&HashMap::from([(
            "claude-test-1".to_string(),
            Tokens {
                input: 1_000_000,
                output: 1_000_000,
                cache_creation: 1_000_000,
                cache_read: 1_000_000,
            },
        )]));
        assert!(!partial && near(usd, 73.5), "{usd}");
        assert!(near(
            estimate(
                &Tokens {
                    output: 1_000_000,
                    ..Default::default()
                },
                "claude-opus-5-5"
            ),
            20.0
        ));
        assert_eq!(estimate_checked(&Tokens::default(), "mystery"), None);
    }

    #[test]
    fn unknown_model_makes_the_figure_partial() {
        let t = Table::default();
        let (usd, partial) = t.usd(&HashMap::from([
            (
                "claude-opus-5-5".to_string(),
                Tokens {
                    output: 1_000_000,
                    ..Default::default()
                },
            ),
            (
                "mystery-model".to_string(),
                Tokens {
                    output: 5_000_000,
                    ..Default::default()
                },
            ),
        ]));
        assert!(partial && near(usd, 20.0));
        assert_eq!(dollars(usd, partial), "≈ $20.00+");
        let (_, partial) = t.usd(&HashMap::from([(
            "claude-haiku-5".to_string(),
            Tokens {
                output: 1,
                ..Default::default()
            },
        )]));
        assert!(partial, "a placeholder price is partial");
    }

    #[test]
    fn overrides() {
        let t = Table::new(&HashMap::from([
            ("claude-opus-5-5".to_string(), "8, 40".to_string()),
            ("bad".to_string(), "x".to_string()),
            ("short".to_string(), "1".to_string()),
        ]));
        let p = t.price("claude-opus-5-5").unwrap();
        assert!(p.input == 8.0 && p.output == 40.0 && near(p.cache_write, 10.0) && near(p.cache_read, 0.8));
        assert!(t.price("bad").is_none() && t.price("short").is_none());
    }

    #[test]
    fn summary_of_account() {
        let t = Table::default();
        let s = t.of(&Account {
            main: HashMap::from([(
                "claude-sonnet-5-5".to_string(),
                Tokens {
                    output: 1_000_000,
                    ..Default::default()
                },
            )]),
            sub: HashMap::from([(
                "claude-haiku-4-5".to_string(),
                Tokens {
                    output: 1_000_000,
                    ..Default::default()
                },
            )]),
            reported: Some(Reported {
                usd: 9.99,
                unknown: true,
            }),
        });
        assert_eq!((s.tokens.total(), s.subagents.total()), (1_000_000, 1_000_000));
        assert!(near(s.usd, 15.0));
        assert_eq!(s.reported_usd, Some(9.99));
        assert!(s.partial);
        assert_eq!(s.line(), "1M tok (+1M subagents) · ≈ $15.00+");
        let sum = s.add(&s);
        assert_eq!((sum.total(), sum.reported_usd), (4_000_000, None));
    }

    #[test]
    fn formats() {
        assert_eq!(tok(950), "950");
        assert_eq!(tok(1200), "1.2k");
        assert_eq!(tok(1000), "1k");
        assert_eq!(tok(12_400), "12k");
        assert_eq!(tok(3_200_000), "3.2M");
        assert_eq!(dollars(0.0, false), "≈ $0");
        assert_eq!(dollars(0.4236, false), "≈ $0.424");
        assert_eq!(dollars(123.4, false), "≈ $123");
        assert_eq!(parse_budget("2M"), Ok(Budget::Tokens(2_000_000)));
        assert_eq!(parse_budget("500k"), Ok(Budget::Tokens(500_000)));
        assert_eq!(parse_budget("$0.50"), Ok(Budget::Usd(0.5)));
        assert!(parse_budget("$x").is_err() && parse_budget("-1").is_err());
    }
}
