"""Shared validation and defaults for attested anonymous OpenCode models."""
from __future__ import annotations
from datetime import datetime, timezone
import re

CATALOG_URL = "https://opencode.ai/zen/v1/models"
PRICING_URL = "https://opencode.ai/docs/zen/"
BASE_URL = "https://opencode.ai/zen/v1"
DEFAULT_MODEL_ID = "ling-3.0-flash-fin-free"
REQUIRED = ("available", "freePricingVerified", "harnessVerified", "inference", "streaming", "toolCall", "toolResult", "anonymous")

def provider_for(api: str) -> str:
    if api not in ("openai-completions", "openai-responses"):
        raise ValueError("unsupported verified free-model protocol")
    return "opencode-free" if api == "openai-completions" else "opencode-free-responses"

def validated_models(report: dict, binary_hash: str | None = None) -> list[dict]:
    if report.get("url") != CATALOG_URL or report.get("pricingSource") != PRICING_URL:
        raise ValueError("free verification requires the official catalog and pricing source")
    modern = report.get("schemaVersion") == 2
    rows = report.get("models") if modern else [report]
    if not isinstance(rows, list):
        raise ValueError("free model evidence has no model rows")
    accepted, seen = [], set()
    for row in rows:
        if modern and row.get("status") != "available":
            continue
        if not all(row.get(key) is True for key in REQUIRED):
            raise ValueError("free model verification is incomplete")
        model = row.get("model")
        if not isinstance(model, str) or not model.strip() or model in seen:
            raise ValueError("invalid or duplicate verified model id")
        seen.add(model)
        digest = row.get("binarySha256")
        if re.fullmatch(r"[a-f0-9]{64}", str(digest)) is None or binary_hash is not None and digest != binary_hash:
            raise ValueError("free model verification belongs to a different runtime")
        verified = datetime.fromisoformat(row["verifiedAt"])
        if verified.tzinfo is None or not -60 <= (datetime.now(timezone.utc) - verified).total_seconds() <= 86400:
            raise ValueError("free model verification must be from the last 24 hours")
        if row.get("pricingSource") != PRICING_URL:
            raise ValueError("free model row lacks official pricing evidence")
        api = row.get("api", "openai-completions")
        provider = provider_for(api)
        if modern:
            proof = row.get("pricingEvidence", {})
            if proof.get("modelId") != model or not proof.get("label") or proof.get("prices") != ["Free", "Free", "Free"]:
                raise ValueError("exact model-to-price-table evidence is missing")
            expected_endpoint = BASE_URL + ("/responses" if api == "openai-responses" else "/chat/completions")
            if proof.get("endpoint") != expected_endpoint:
                raise ValueError("free pricing evidence does not match the inference protocol")
            if row.get("harnessModel") != model or row.get("provider") != provider or row.get("harnessToolResult") is not True or row.get("harnessCompleted") is not True:
                raise ValueError("free model harness attestation does not match its route")
        elif model != DEFAULT_MODEL_ID:
            raise ValueError("legacy evidence is supported only for the original Ling route")
        accepted.append({**row, "api": api, "provider": provider})
    if not accepted:
        raise ValueError("no free model passed all release checks")
    if modern:
        expected = [{"provider": row["provider"], "model": row["model"]} for row in accepted]
        if report.get("includedModels") != expected or report.get("defaultModel") not in expected:
            raise ValueError("free package selection differs from its verified models")
        if report.get("binarySha256") != accepted[0]["binarySha256"] or any(row["binarySha256"] != report["binarySha256"] for row in accepted):
            raise ValueError("free evidence mixes different runtime builds")
    return accepted

def package_defaults(report: dict, binary_hash: str) -> dict:
    rows = validated_models(report, binary_hash)
    providers = {}
    for row in rows:
        provider = providers.setdefault(row["provider"], {"displayName": "OpenCode 免费模型" + (" · Responses" if row["api"] == "openai-responses" else ""), "keyless": True, "api": row["api"], "baseURL": BASE_URL, "models": []})
        model = {"id": row["model"], "name": row.get("name") or row.get("pricingLabel") or row["model"], "maxTokens": row.get("maxTokens", 16384)}
        for key in ("contextWindow", "reasoningEfforts", "input"):
            if key in row:
                model[key] = row[key]
        if report.get("schemaVersion") != 2:
            model.update(contextWindow=262144, reasoningEfforts=False)
        provider["models"].append(model)
    selected = report.get("defaultModel") or {"provider": rows[0]["provider"], "model": rows[0]["model"]}
    return {"llm-pi-ai": {"providers": providers}, "agent-default-model": selected}
