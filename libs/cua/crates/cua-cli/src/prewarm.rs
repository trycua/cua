//! `cua run --compat P --prewarm IMAGE[=N]`: what to keep warm, and for how
//! long.
//!
//! The mechanism is core (`Spaces::prewarm`, the elastic placement's warm
//! pools); this module is the thin part: parsing the flag, resolving what a
//! provider's template, snapshot or image reference starts from (the
//! providers' own helpers in `cua_compat`), choosing the count, and keeping
//! the prewarm for exactly the life of the run behind a [`Prewarmer`].
//!
//! `IMAGE` is whatever the program will name when it creates a sandbox: an
//! E2B template, a Daytona snapshot or image, a Modal image, or a Cua image
//! reference; empty (`--prewarm` alone, or `--prewarm =4`) is the provider's
//! default template. `N` is the expected parallelism; without it the count is
//! `--parallel` (default 1).

use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap};

/// One `--prewarm` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    /// What the program names; empty: the provider's default.
    pub reference: String,
    /// The expected parallelism, when given.
    pub count: Option<u32>,
}

/// Most Spaces one prewarm keeps (the core limit).
pub const MAX_COUNT: u32 = 256;

/// Parses `IMAGE[=N]`. A reference may itself contain `=` only before a
/// numeric tail (`name=4`); everything else is the reference.
pub fn parse(value: &str) -> Result<Spec, String> {
    let v = value.trim();
    let (reference, count) = match v.rsplit_once('=') {
        Some((r, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            let n: u32 = n
                .parse()
                .map_err(|_| format!("--prewarm {value}: {n} is not a count"))?;
            if n == 0 || n > MAX_COUNT {
                return Err(format!("--prewarm {value}: the count is 1 to {MAX_COUNT}"));
            }
            (r, Some(n))
        }
        Some((_, "")) => {
            return Err(format!("--prewarm {value}: a count must follow the ="));
        }
        _ => (v, None),
    };
    Ok(Spec {
        reference: reference.trim().to_string(),
        count,
    })
}

/// What to keep warm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The Cua image (`None`: the Space default image).
    pub image: Option<String>,
    pub count: u32,
}

/// Resolves the specs of a run for `provider` into Cua images and counts,
/// merging entries that resolve to the same image (their counts add up, up to
/// the core limit). `parallel` is the count for a spec that gives none.
pub fn plan(provider: &str, specs: &[Spec], parallel: Option<u32>) -> Result<Vec<Plan>, String> {
    let default_count = parallel.unwrap_or(1).clamp(1, MAX_COUNT);
    let mut out: Vec<Plan> = Vec::new();
    for s in specs {
        let image = resolve(provider, &s.reference)?;
        let count = s.count.unwrap_or(default_count);
        match out.iter_mut().find(|p| p.image == image) {
            Some(p) => p.count = (p.count + count).min(MAX_COUNT),
            None => out.push(Plan { image, count }),
        }
    }
    Ok(out)
}

fn resolve(provider: &str, reference: &str) -> Result<Option<String>, String> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "e2b" => cua_compat::e2b::prewarm::resolve_image(
            reference,
            &BTreeMap::new().into_iter().collect(),
        ),
        "daytona" => cua_compat::daytona::prewarm::resolve(reference, &[], "", (1, 1024))
            .map(|t| Some(t.cua_image).filter(|i| !i.is_empty())),
        "modal" => cua_compat::modal::prewarm::resolve_image(reference, &HashMap::new(), "")
            .map(|t| t.cua_image),
        other => Err(format!("--prewarm: unknown provider {other}")),
    }
}

/// The core prewarm, as `Spaces::prewarm` and `remove_prewarm` offer it.
#[async_trait]
pub trait Prewarmer: Send + Sync {
    /// Keeps `count` Spaces of `image` ready; returns the prewarm's id.
    async fn prewarm(
        &self,
        image: Option<&str>,
        count: u32,
        on: Option<&str>,
        ttl_secs: u64,
    ) -> Result<String, String>;
    async fn remove(&self, id: &str) -> Result<(), String>;
}

/// Runs `run` with the plans prewarmed, and removes the prewarms when it
/// ends, whatever way it ends. A prewarm that is refused (budget, quota,
/// pins) fails the run before it starts, with the core's reason, after
/// removing the ones made so far.
pub async fn with_prewarm<T, F>(
    p: &dyn Prewarmer,
    plans: &[Plan],
    on: Option<&str>,
    ttl_secs: u64,
    run: F,
) -> Result<T, String>
where
    F: std::future::Future<Output = T>,
{
    let mut ids = Vec::new();
    for plan in plans {
        match p
            .prewarm(plan.image.as_deref(), plan.count, on, ttl_secs)
            .await
        {
            Ok(id) => ids.push(id),
            Err(e) => {
                for id in &ids {
                    let _ = p.remove(id).await;
                }
                return Err(e);
            }
        }
    }
    let out = run.await;
    for id in &ids {
        if let Err(e) = p.remove(id).await {
            eprintln!("cua: prewarm {id} was not removed (it ends at its ttl): {e}");
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn parses_image_and_count() {
        let s = |r: &str, c| Spec {
            reference: r.into(),
            count: c,
        };
        assert_eq!(parse("base").unwrap(), s("base", None));
        assert_eq!(parse("base=16").unwrap(), s("base", Some(16)));
        assert_eq!(parse("").unwrap(), s("", None));
        assert_eq!(parse("=4").unwrap(), s("", Some(4)));
        assert_eq!(
            parse("ghcr.io/trycua/linux:24.04=8").unwrap(),
            s("ghcr.io/trycua/linux:24.04", Some(8))
        );
        assert!(parse("base=0").is_err());
        assert!(parse("base=999").is_err());
        assert!(parse("base=").is_err());
    }

    #[test]
    fn plans_map_each_providers_names_and_add_up() {
        let specs = vec![
            parse("base=3").unwrap(),
            parse("code-interpreter-v1").unwrap(),
            parse("ghcr.io/trycua/linux:24.04=2").unwrap(),
        ];
        let p = plan("e2b", &specs, Some(5)).unwrap();
        // base and code-interpreter-v1 are the same default image: 3 + 5.
        assert_eq!(
            p,
            vec![
                Plan {
                    image: None,
                    count: 8
                },
                Plan {
                    image: Some("ghcr.io/trycua/linux:24.04".into()),
                    count: 2
                }
            ]
        );
        // No count and no hint: one.
        assert_eq!(
            plan("e2b", &[parse("").unwrap()], None).unwrap()[0].count,
            1
        );
        // A name the provider cannot start from is refused with the reason.
        assert!(plan("e2b", &[parse("my-built-template").unwrap()], None).is_err());
        assert!(plan("daytona", &[parse("base").unwrap()], None).is_ok());
        assert!(plan("modal", &[parse("python:3.12").unwrap()], Some(2)).is_ok());
        assert!(plan("nope", &[parse("base").unwrap()], None).is_err());
    }

    #[derive(Default)]
    struct Fake {
        log: Mutex<Vec<String>>,
        refuse_second: bool,
    }

    #[async_trait]
    impl Prewarmer for Fake {
        async fn prewarm(
            &self,
            image: Option<&str>,
            count: u32,
            on: Option<&str>,
            ttl: u64,
        ) -> Result<String, String> {
            let mut l = self.log.lock().unwrap();
            if self.refuse_second && l.len() == 1 {
                return Err("over the monthly budget".into());
            }
            l.push(format!("prewarm {image:?} x{count} on {on:?} ttl {ttl}"));
            Ok(format!("pw-{}", l.len()))
        }
        async fn remove(&self, id: &str) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("remove {id}"));
            Ok(())
        }
    }

    #[tokio::test]
    async fn prewarms_exist_for_exactly_the_run() {
        let f = Fake::default();
        let plans = vec![
            Plan {
                image: None,
                count: 4,
            },
            Plan {
                image: Some("img".into()),
                count: 1,
            },
        ];
        let out = with_prewarm(&f, &plans, Some("local"), 7200, async {
            f.log.lock().unwrap().push("run".into());
            7
        })
        .await
        .unwrap();
        assert_eq!(out, 7);
        let l = f.log.lock().unwrap().clone();
        assert_eq!(
            l,
            [
                "prewarm None x4 on Some(\"local\") ttl 7200",
                "prewarm Some(\"img\") x1 on Some(\"local\") ttl 7200",
                "run",
                "remove pw-1",
                "remove pw-2"
            ]
        );
    }

    #[tokio::test]
    async fn a_refused_prewarm_fails_the_run_and_cleans_up() {
        let f = Fake {
            refuse_second: true,
            ..Default::default()
        };
        let plans = vec![
            Plan {
                image: None,
                count: 4,
            },
            Plan {
                image: Some("img".into()),
                count: 1,
            },
        ];
        let r = with_prewarm(&f, &plans, None, 60, async {
            unreachable!("the run must not start")
        })
        .await;
        assert_eq!(r.unwrap_err(), "over the monthly budget");
        assert_eq!(f.log.lock().unwrap().last().unwrap(), "remove pw-1");
    }
}
