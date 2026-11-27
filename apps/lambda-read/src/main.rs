//! Lambda `bootstrap` for the read path: configuration from the process environment, then the
//! Runtime API loop. A returned error ends the process so Lambda restarts the environment.

fn main() -> Result<(), lambda_read::StartError> {
    let config = lambda_read::Config::from_lookup(|name| std::env::var(name).ok())?;
    lambda_read::start(config)
}
