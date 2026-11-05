#![allow(clippy::wildcard_imports)]
use super::*;

pub fn profile_inspect(arguments: &ProfileInspect) -> Result<()> {
    let config_path = arguments.source.resolve_config_path()?;
    let config = helpers::load(&config_path)?;
    let profile = config.profile(&arguments.profile)?;
    let materials = config.profile_materials(profile)?;
    if arguments.json {
        let mut value = serde_json::to_value(profile)?;
        value["effective_materials"] = serde_json::to_value(&materials)?;
        helpers::print_json(&value)?;
    } else {
        println!("profile: {} ({})", profile.id, profile.display_name);
        println!("tags: {}", profile.tags.join(", "));
        println!("repository: {}", config.profile_repository(profile));
        let provider = if profile.adapter_type == "pi" {
            profile.provider.as_deref().unwrap_or("missing")
        } else {
            "adapter-managed"
        };
        println!("provider: {provider}");
        if profile.adapter_type == "pi"
            && let Some(provider_id) = profile.provider.as_deref()
            && let Ok(llm) = config.llm_provider_for(provider_id)
        {
            let credential = llm.resolved_credential();
            println!("credential: {:?}", credential.kind);
        }
        println!("model: {}", profile.model.as_deref().unwrap_or("provider default"));
        println!("reasoning: {}", profile.reasoning.as_deref().unwrap_or("provider default"));
        println!("workspace: {}", profile.workspace().display());
        println!("effective materials: {}", materials.digest);
        println!("profile revision: {}", materials.revision);
        println!("native material source: {}", materials.source.as_deref().unwrap_or("none"));
        for (path, digest) in &materials.files {
            println!("native material: {path} sha256={digest}");
        }
        for skill in &materials.skills {
            println!("skill: {} sha256={}", skill.name, skill.digest);
        }
        println!("status surfaces: {}", profile.status_surfaces.join(", "));
        println!(
            "context budget: {:.0}% / {} bytes hard",
            profile.github_context_soft_ratio * 100.0,
            profile.github_context_hard_bytes
        );
        println!("user instructions:\n{}", profile.user_instructions);
    }
    Ok(())
}
