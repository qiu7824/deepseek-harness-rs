use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Role {
    pub id: String,
    pub name: String,
    #[serde(default)] pub instructions: String,
    #[serde(default)] pub provider: String,
    #[serde(default)] pub model: String,
    #[serde(default)] pub reasoning_effort: String,
    #[serde(default)] pub max_tokens: Option<u64>,
    #[serde(default)] pub allow_tools: Vec<String>,
    #[serde(default)] pub can_spawn: bool,
}

impl Role {
    pub fn validate(&self) -> Result<(), String> {
        if !super::name(&self.id) || self.name.trim().is_empty() || self.name.len() > 128
            || self.instructions.len() > 16_384 || self.provider.len() > 200 || self.model.len() > 300
            || self.reasoning_effort.len() > 64 || self.provider.is_empty() != self.model.is_empty()
            || self.max_tokens.is_some_and(|n| n == 0 || n > 1_000_000)
            || self.allow_tools.len() > 128 || self.allow_tools.iter().any(|s| s.is_empty() || s.len() > 200)
        { return Err("invalid collaboration role, model route or tool list".into()); }
        Ok(())
    }
    pub fn agent_options(&self) -> dsh_agent::AgentOptions {
        dsh_agent::AgentOptions {
            provider: (!self.provider.is_empty()).then(|| self.provider.clone()),
            model: (!self.model.is_empty()).then(|| self.model.clone()),
            reasoning_effort: (!self.reasoning_effort.is_empty()).then(|| dsh_llm::reasoning_effort_id(&self.reasoning_effort)),
            max_tokens: self.max_tokens,
            ..Default::default()
        }
    }
    pub fn tool_filter(&self) -> Option<dsh_tools::ToolRestriction> {
        let allow=(!self.allow_tools.is_empty()).then(||self.allow_tools.clone());
        // Keep task/message coordination available; AgentTeams separately
        // rejects member creation by non-lead actors. Restrict every built-in
        // delegation tool at invocation as well as in the advertised schema.
        let deny=(!self.can_spawn).then(||["subagent","subagent_fork","subagent_codex","subagent_claude_code"].map(str::to_owned).to_vec());
        (allow.is_some()||deny.is_some()).then_some(dsh_tools::ToolRestriction{allow,deny})
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub roles: Vec<Role>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub max_members: usize,
    pub default_mode: String,
    pub show_button: bool,
    pub default_profile: String,
    pub profiles: Vec<Profile>,
}
impl Default for Config {
    fn default() -> Self {
        Self { enabled: false, max_members: 8, default_mode: "off".into(), show_button: true, default_profile: String::new(), profiles: vec![] }
    }
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=16).contains(&self.max_members) || !matches!(self.default_mode.as_str(), "off" | "auto" | "custom") || self.profiles.len() > 16 {
            return Err("invalid collaboration defaults".into());
        }
        let mut ids = BTreeSet::new();
        for profile in &self.profiles {
            if !super::name(&profile.id) || !ids.insert(&profile.id) || profile.name.trim().is_empty() || profile.name.len() > 128
                || profile.roles.is_empty() || profile.roles.len() > 16 { return Err("invalid or duplicate collaboration profile".into()); }
            let mut roles = BTreeSet::new();
            for role in &profile.roles { role.validate()?; if !roles.insert(&role.id) { return Err("duplicate collaboration role".into()); } }
        }
        if !self.default_profile.is_empty() && !ids.contains(&self.default_profile) { return Err("default collaboration profile does not exist".into()); }
        if self.default_mode == "custom" && self.default_profile.is_empty() { return Err("custom collaboration requires a default profile".into()); }
        Ok(())
    }
    pub fn parse(mut value: serde_json::Value) -> Result<Self, String> {
        // Settings schemas represent numeric values as f64, including integers.
        fn integer(value: &mut serde_json::Value, maximum: u64) -> Result<(), String> {
            if value.is_null() { return Ok(()); }
            let number=value.as_f64().filter(|n|n.is_finite()&&n.fract()==0.0&&*n>=1.0&&*n<=maximum as f64).ok_or("collaboration limits must be positive integers within their bounds")?;
            *value=serde_json::json!(number as u64); Ok(())
        }
        if let Some(limit)=value.get_mut("maxMembers"){integer(limit,16)?;}
        if let Some(profiles)=value.get_mut("profiles").and_then(|v|v.as_array_mut()) {
            for profile in profiles {if let Some(roles)=profile.get_mut("roles").and_then(|v|v.as_array_mut()) {
                for role in roles {if let Some(limit)=role.get_mut("maxTokens"){integer(limit,1_000_000)?;}}
            }}
        }
        let config: Self = serde_json::from_value(value).map_err(|e| e.to_string())?;
        config.validate()?;
        Ok(config)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionConfig {
    pub revision: u64,
    pub mode: String,
    pub profile: Option<Profile>,
    #[serde(default = "explicit_configuration")]
    pub explicit: bool,
}
fn explicit_configuration() -> bool { true }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_settings_and_role_routes_are_validated() {
        let old = Config::parse(serde_json::json!({"enabled":true,"maxMembers":4})).unwrap();
        assert!(old.enabled); assert_eq!(old.default_mode, "off");
        assert_eq!(Config::parse(serde_json::json!({"maxMembers":8.0})).unwrap().max_members,8);
        assert!(Config::parse(serde_json::json!({"maxMembers":8.5})).is_err());
        assert!(Config::parse(serde_json::json!({"defaultMode":"custom"})).is_err());
        let mut role = Role { id:"reviewer".into(), name:"审查".into(), provider:"route".into(), ..Default::default() };
        assert!(role.validate().is_err()); role.model="model-a".into(); role.max_tokens=Some(2048); role.validate().unwrap();
        let options=role.agent_options(); assert_eq!(options.provider.as_deref(),Some("route")); assert_eq!(options.max_tokens,Some(2048));
        assert!(role.tool_filter().unwrap().deny.unwrap().contains(&"subagent".to_owned()));
    }
    #[test]
    fn profile_ids_and_defaults_cannot_drift() {
        let profile=Profile{id:"coding".into(),name:"编码".into(),roles:vec![Role{id:"builder".into(),name:"实现".into(),..Default::default()}]};
        let mut config=Config{profiles:vec![profile.clone()],default_profile:"coding".into(),default_mode:"custom".into(),..Default::default()};
        config.validate().unwrap(); config.profiles.push(profile); assert!(config.validate().is_err());
    }
}
