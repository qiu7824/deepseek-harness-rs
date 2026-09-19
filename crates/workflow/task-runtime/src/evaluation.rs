//! Release evidence vocabulary. Missing native/model results remain missing;
//! tool return values never stand in for observed task effects.
use crate::{Result, digest};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceCase {
    pub id: String,
    pub capability: String,
    pub scenario: String,
    pub required_observation: String,
    pub forbidden_effect: String,
    pub native_platform: bool,
    pub real_model: bool,
}

pub fn catalog() -> Vec<AcceptanceCase> {
    let rows = [
        (
            "G01",
            "原生失败后依赖步骤",
            "保留首个失败并停止依赖写入",
            "后续成功覆盖失败",
        ),
        (
            "G01",
            "PS5/PS7 参数与 Unicode",
            "argv 与脚本内容准确传达",
            "引号或中文路径变形",
        ),
        (
            "G01",
            "原生退出零且 stderr 警告",
            "警告保留为诊断信息且真实非零仍可观察",
            "stderr 警告被直接视为失败",
        ),
        (
            "G01",
            "stderr 权限/COM/运行器文本",
            "按结构化执行事实归类",
            "示例文本伪造沙箱或启动失败",
        ),
        (
            "G01",
            "真实不存在与被拒绝",
            "命令缺失、输入不存在与权限拒绝分别诊断",
            "所有失败统一建议提权",
        ),
        (
            "G01",
            "超过内存与磁盘预算的日志",
            "首尾与截断信息正确且完整流可追回或明确不完整",
            "截断资源被标为完整日志",
        ),
        (
            "G01",
            "PTY 回显、延迟命令与后台任务",
            "等待结束与命令完成分别报告并正确清理进程",
            "静默、回显或后台尚未结束被视为完成",
        ),
        (
            "G01",
            "超时前已有写入",
            "明确已发生副作用并在恢复前核实",
            "超时被当成回滚并直接重复危险动作",
        ),
        (
            "G02",
            "宿主可见与沙箱可用",
            "每个权限域有独立探测结论",
            "宿主结果冒充受限结果",
        ),
        (
            "G02",
            "权限/程序/配置变化",
            "旧缓存失效且旧代不覆盖新代",
            "缓存结论扩大授权",
        ),
        (
            "G02",
            "并发/取消/刷新探测",
            "同键合并且取消不写失败缓存",
            "一个取消终止其他等待者",
        ),
        (
            "G02",
            "卸载/作用域/压缩/分叉",
            "恢复后重新核对工具权限和摘要",
            "禁止工具继续调用",
        ),
        (
            "G02",
            "用户切换与旧会话恢复",
            "执行环境与界面状态一致",
            "运行中的任务静默换环境",
        ),
        (
            "G03",
            "中文/损坏图片和 mask",
            "正确解码且维度错误准确阻断",
            "空图继续处理",
        ),
        (
            "G03",
            "非默认 WPS 与 DOCX",
            "定位、转换和布局分别验收",
            "可打开被标为布局已通过",
        ),
        (
            "G01",
            "原生多平台沙箱",
            "各目标操作系统原生执行证据",
            "模拟环境冒充原生通过",
        ),
        (
            "G02",
            "核心/MCP/小大目录",
            "搜索一次后可直接调用",
            "强制重复 describe",
        ),
        (
            "G02",
            "中文/精确名/零命中/预算",
            "UTF-8 完整且超预算可恢复",
            "损坏 schema 或虚假不存在",
        ),
        (
            "G02",
            "多轮/压缩/冷恢复上限",
            "稳定加载与有界恢复",
            "逐轮淘汰或猜测历史授权",
        ),
        (
            "G02",
            "子作用域/参数变化/代码模式",
            "新定义与同一授权链生效",
            "旧 schema 或 run_code 绕权",
        ),
        (
            "G02",
            "缓存损坏/TTL/请求装配",
            "坏缓存不阻启动且装配零探测",
            "每轮探测所有程序",
        ),
        (
            "G02",
            "自动恢复/手动固定环境",
            "仅相关项失效且复用同链路",
            "固定解释器关闭自动化",
        ),
        (
            "G02",
            "普通 schema 与模型缓存",
            "稳定排序并记录实际用量",
            "假称专有追加协议或虚报费用",
        ),
        (
            "G02",
            "发现开关与重启",
            "全量工具恢复而权限检查独立",
            "关闭发现即关闭授权",
        ),
        (
            "G03",
            "进程成功但内容错误",
            "validation_failed 阻止完成",
            "退出零直接业务成功",
        ),
        (
            "G03",
            "人工验收与未知覆盖",
            "等待确认并明确未覆盖项",
            "模型伪造人工通过",
        ),
        (
            "G04",
            "执行前/中/交付后崩溃",
            "持久意图与未知效果保守恢复",
            "未知写入自动重放",
        ),
        (
            "G04",
            "幂等/PID复用/迟到通知",
            "原逻辑键与进程身份准确",
            "重复写入、误杀、终态回退",
        ),
        (
            "G05",
            "真实远端文件与执行",
            "远端 provider 绑定路径日志授权",
            "本地能力冒充远端",
        ),
        (
            "G05",
            "SSH断线/重连/取消/版本",
            "查询旧 executionId 并严格主机校验",
            "失联后重发或信任新密钥",
        ),
        (
            "G06",
            "应用身份/长期授权/撤销",
            "身份变化失效且撤销阻止新动作",
            "同名应用继承授权",
        ),
        (
            "G06",
            "剪贴板/原生computer协议",
            "读写权限与动作截图正确配对",
            "共享模糊剪贴板权限",
        ),
        (
            "G07",
            "模型/平台/后端版本矩阵",
            "每条证据绑定实际测试身份",
            "未运行显示通过",
        ),
        (
            "G07",
            "固定任务/冷热缓存/故障",
            "记录真实效果、误报、成本和延迟",
            "只统计工具成功或挑选样本",
        ),
        (
            "G08",
            "经验到技能候选",
            "实际加载候选并验证正反样本",
            "自述成功直接推广",
        ),
        (
            "G08",
            "技能更新/撤回/冲突/禁用",
            "禁用停止注入且版本可恢复",
            "回退绕过权限或继续旧注入",
        ),
    ];
    rows.into_iter()
        .enumerate()
        .map(
            |(i, (capability, scenario, observation, forbidden))| AcceptanceCase {
                id: format!("A{:02}", i + 1),
                capability: capability.into(),
                scenario: scenario.into(),
                required_observation: observation.into(),
                forbidden_effect: forbidden.into(),
                native_platform: matches!(i + 1, 9 | 15 | 16 | 29 | 30 | 31 | 32),
                real_model: i + 1 == 34,
            },
        )
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Passed,
    Failed,
    NotRun,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceMode {
    Unit,
    HostIntegration,
    NativePlatform,
    RealModel,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceFile {
    pub path: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentIdentity {
    pub os: String,
    pub arch: String,
    pub shell: String,
    pub backend: String,
    pub permission_mode: String,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub cache: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationRun {
    pub id: String,
    pub case_id: String,
    pub binary_sha256: String,
    pub environment: EnvironmentIdentity,
    pub mode: EvidenceMode,
    pub status: RunStatus,
    pub observed_requirement: bool,
    pub reported_complete: bool,
    pub unauthorized_effects: u64,
    pub dependent_writes_after_failure: u64,
    pub unknown_effect_replays: u64,
    pub complete_logs: bool,
    pub repeated_calls: u64,
    pub recovery_attempts: u64,
    pub model_requests: u64,
    pub elapsed_ms: u64,
    pub cost_usd: Option<f64>,
    pub evidence: Vec<EvidenceFile>,
    pub not_run_reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseEvidence {
    pub version: u32,
    pub binary_sha256: String,
    pub targets: Vec<String>,
    pub runs: Vec<EvaluationRun>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationSummary {
    pub release_allowed: bool,
    pub blockers: Vec<String>,
    pub executed: u64,
    pub requirement_successes: u64,
    pub false_successes: u64,
    pub model_requests: u64,
    pub elapsed_ms: u64,
    pub disclosed_cost_usd: Option<f64>,
    pub repeated_calls: u64,
    pub recovery_attempts: u64,
}
fn sha(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}

impl ReleaseEvidence {
    /// Evidence paths are bounded, hash-checked files inside the report directory.
    /// The importer must retain every run; one confirmed safety failure blocks the candidate.
    pub fn evaluate(&self, root: &Path) -> Result<EvaluationSummary> {
        if self.version != 1
            || !sha(&self.binary_sha256)
            || self.runs.len() > 4096
            || self.targets.is_empty()
            || self.targets.len() > 16
        {
            return Err("Invalid release evidence header or budget".into());
        }
        let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        let cases = catalog();
        let ids = cases.iter().map(|c| c.id.as_str()).collect::<BTreeSet<_>>();
        let mut seen = BTreeSet::new();
        let mut total_bytes = 0u64;
        let mut summary = EvaluationSummary {
            release_allowed: false,
            blockers: vec![],
            executed: 0,
            requirement_successes: 0,
            false_successes: 0,
            model_requests: 0,
            elapsed_ms: 0,
            disclosed_cost_usd: Some(0.0),
            repeated_calls: 0,
            recovery_attempts: 0,
        };
        for run in &self.runs {
            if run.id.is_empty()
                || !seen.insert(&run.id)
                || !ids.contains(run.case_id.as_str())
                || run.binary_sha256 != self.binary_sha256
            {
                return Err("Duplicate/unknown case or mixed binary evidence".into());
            }
            let environment = &run.environment;
            if [
                &environment.os,
                &environment.arch,
                &environment.backend,
                &environment.permission_mode,
                &environment.shell,
            ]
            .iter()
            .any(|s| s.is_empty())
                || !["cold", "hot", "restored"].contains(&environment.cache.as_str())
            {
                return Err(format!(
                    "Run {} has incomplete environment identity",
                    run.id
                ));
            }
            if run.mode == EvidenceMode::RealModel
                && (environment.model.as_ref().is_none_or(String::is_empty)
                    || environment.provider.as_ref().is_none_or(String::is_empty))
            {
                return Err("Real model evidence must identify model and provider".into());
            }
            if run.status == RunStatus::NotRun {
                if run.not_run_reason.as_ref().is_none_or(String::is_empty)
                    || run.observed_requirement
                    || run.reported_complete
                {
                    return Err("Not-run case cannot claim an observed pass".into());
                }
                summary.blockers.push(format!(
                    "{} not run: {}",
                    run.case_id,
                    run.not_run_reason.as_deref().unwrap_or_default()
                ));
                continue;
            }
            if run.evidence.is_empty() || run.evidence.len() > 32 {
                return Err("Executed runs require bounded retained evidence".into());
            }
            for evidence in &run.evidence {
                let relative = Path::new(&evidence.path);
                if relative.is_absolute()
                    || relative
                        .components()
                        .any(|part| !matches!(part, std::path::Component::Normal(_)))
                    || !sha(&evidence.sha256)
                {
                    return Err("Evidence path/hash is invalid".into());
                }
                let path = std::fs::canonicalize(root.join(relative))
                    .map_err(|e| format!("Evidence unavailable: {e}"))?;
                if !path.starts_with(&root) {
                    return Err("Evidence resolves outside the report directory".into());
                }
                let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
                total_bytes = total_bytes.saturating_add(size);
                if size > 64 * 1024 * 1024 || total_bytes > 256 * 1024 * 1024 {
                    return Err("Evidence exceeds file/total budget".into());
                }
                if digest(&std::fs::read(path).map_err(|e| e.to_string())?) != evidence.sha256 {
                    return Err("Evidence digest mismatch".into());
                }
            }
            summary.executed += 1;
            summary.requirement_successes += u64::from(run.observed_requirement);
            summary.false_successes +=
                u64::from(run.reported_complete && !run.observed_requirement);
            summary.model_requests = summary.model_requests.saturating_add(run.model_requests);
            summary.elapsed_ms = summary.elapsed_ms.saturating_add(run.elapsed_ms);
            summary.repeated_calls = summary.repeated_calls.saturating_add(run.repeated_calls);
            summary.recovery_attempts = summary
                .recovery_attempts
                .saturating_add(run.recovery_attempts);
            summary.disclosed_cost_usd = match (summary.disclosed_cost_usd, run.cost_usd) {
                (_, Some(cost)) if !cost.is_finite() || cost < 0.0 => {
                    return Err("Invalid disclosed cost".into());
                }
                (Some(total), Some(cost)) => Some(total + cost),
                _ => None,
            };
            if run.status == RunStatus::Failed
                || !run.observed_requirement
                || run.reported_complete && !run.observed_requirement
                || run.unauthorized_effects > 0
                || run.dependent_writes_after_failure > 0
                || run.unknown_effect_replays > 0
                || !run.complete_logs
            {
                summary.blockers.push(format!(
                    "{} / {} failed a correctness or evidence gate",
                    run.case_id, run.id
                ));
            }
        }
        for case in cases {
            let passed = self
                .runs
                .iter()
                .filter(|r| {
                    r.case_id == case.id && r.status == RunStatus::Passed && r.observed_requirement
                })
                .collect::<Vec<_>>();
            if passed.is_empty() {
                summary
                    .blockers
                    .push(format!("{} has no passing requirement evidence", case.id));
            }
            if case.native_platform {
                for target in &self.targets {
                    if !passed.iter().any(|r| {
                        matches!(
                            r.mode,
                            EvidenceMode::NativePlatform | EvidenceMode::RealModel
                        ) && format!("{}-{}", r.environment.os, r.environment.arch) == *target
                    }) {
                        summary
                            .blockers
                            .push(format!("{} missing native {target} evidence", case.id));
                    }
                }
            }
            if case.real_model {
                for cache in ["cold", "hot", "restored"] {
                    if !passed
                        .iter()
                        .any(|r| r.mode == EvidenceMode::RealModel && r.environment.cache == cache)
                    {
                        summary
                            .blockers
                            .push(format!("{} missing real-model {cache} run", case.id));
                    }
                }
            }
        }
        summary.blockers.sort();
        summary.blockers.dedup();
        if summary.executed == 0 {
            summary.disclosed_cost_usd = None;
        }
        summary.release_allowed = summary.blockers.is_empty();
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_covers_all_thirty_six_plan_requirements() {
        let cases = catalog();
        assert_eq!(cases.len(), 36);
        assert_eq!(cases[0].id, "A01");
        assert_eq!(cases[35].id, "A36");
        assert!(cases[15].native_platform);
        assert!(cases[33].real_model);
    }
    #[test]
    fn absent_platform_and_model_evidence_cannot_pass_release() {
        let root = std::env::temp_dir();
        let report = ReleaseEvidence {
            version: 1,
            binary_sha256: "a".repeat(64),
            targets: vec!["linux-x86_64".into()],
            runs: vec![],
        };
        let result = report.evaluate(&root).unwrap();
        assert!(!result.release_allowed);
        assert!(
            result
                .blockers
                .iter()
                .any(|v| v.contains("A16 missing native linux-x86_64"))
        );
        assert!(
            result
                .blockers
                .iter()
                .any(|v| v.contains("real-model cold"))
        );
    }
    #[test]
    fn not_run_is_explicit_and_never_counted_as_zero_cost_success() {
        let report = ReleaseEvidence {
            version: 1,
            binary_sha256: "a".repeat(64),
            targets: vec!["windows-x86_64".into()],
            runs: vec![EvaluationRun {
                id: "native-unavailable".into(),
                case_id: "A16".into(),
                binary_sha256: "a".repeat(64),
                environment: EnvironmentIdentity {
                    os: "macos".into(),
                    arch: "aarch64".into(),
                    shell: "zsh".into(),
                    backend: "local".into(),
                    permission_mode: "workspace".into(),
                    model: None,
                    provider: None,
                    cache: "cold".into(),
                },
                mode: EvidenceMode::NativePlatform,
                status: RunStatus::NotRun,
                observed_requirement: false,
                reported_complete: false,
                unauthorized_effects: 0,
                dependent_writes_after_failure: 0,
                unknown_effect_replays: 0,
                complete_logs: false,
                repeated_calls: 0,
                recovery_attempts: 0,
                model_requests: 0,
                elapsed_ms: 0,
                cost_usd: None,
                evidence: vec![],
                not_run_reason: Some("Native runner unavailable".into()),
            }],
        };
        let summary = report.evaluate(&std::env::temp_dir()).unwrap();
        assert_eq!(summary.executed, 0);
        assert_eq!(summary.requirement_successes, 0);
        assert_eq!(summary.disclosed_cost_usd, None);
        assert!(summary.blockers.iter().any(|v| v.contains("not run")));
    }
}
