from __future__ import annotations
import io
import json
import pathlib
import sys
import tempfile
import unittest
import urllib.error
from datetime import datetime, timezone, timedelta
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import free_model_evidence as evidence
import verify_free_model_catalog as verifier

def attested(model="proven-free", api="openai-completions"):
    return {"model":model,"name":model,"api":api,"provider":evidence.provider_for(api),"status":"available",
            "verifiedAt":datetime.now(timezone.utc).isoformat(),"binarySha256":"a"*64,"pricingSource":evidence.PRICING_URL,
            "pricingEvidence":{"modelId":model,"label":model,"prices":["Free"]*3,"endpoint":evidence.BASE_URL+("/responses" if api=="openai-responses" else "/chat/completions")},
            "harnessModel":model,"harnessToolResult":True,"harnessCompleted":True,"maxTokens":16384,
            **{key:True for key in evidence.REQUIRED}}

def report(rows):
    included=[{"provider":row["provider"],"model":row["model"]} for row in rows if row.get("status")=="available"]
    return {"schemaVersion":2,"url":evidence.CATALOG_URL,"pricingSource":evidence.PRICING_URL,"binarySha256":"a"*64,
            "models":rows,"includedModels":included,"defaultModel":included[0] if included else None}

class FreeEvidenceTests(unittest.TestCase):
    def test_price_proof_joins_exact_id_to_all_three_free_price_columns(self):
        rows=[["Good","opaque-id",evidence.BASE_URL+"/chat/completions"],["Good","Free","Free","Free","-"],
              ["Paid","looks-free",evidence.BASE_URL+"/chat/completions"],["Paid","Free","$1","Free","-"],
              ["Other","wrong-id",evidence.BASE_URL+"/responses"],["Different","Free","Free","Free","-"]]
        self.assertEqual(set(verifier.pricing_catalog_from_rows(rows)),{"opaque-id"})

    def test_package_includes_only_attested_models_and_keeps_protocols_separate(self):
        rows=[attested(),attested("response-free","openai-responses"),{"model":"limited-free","status":"rate-limited"}]
        defaults=evidence.package_defaults(report(rows),"a"*64)
        providers=defaults["llm-pi-ai"]["providers"]
        self.assertEqual(set(providers),{"opencode-free","opencode-free-responses"})
        self.assertEqual(providers["opencode-free-responses"]["api"],"openai-responses")
        self.assertEqual(providers["opencode-free"]["models"][0]["id"],"proven-free")
        self.assertNotIn("contextWindow",providers["opencode-free"]["models"][0])
        self.assertNotIn("limited-free",json.dumps(defaults))

    def test_stale_mismatched_and_missing_exact_pricing_evidence_fail_closed(self):
        for change in ({"binarySha256":"b"*64},{"verifiedAt":(datetime.now(timezone.utc)-timedelta(days=2)).isoformat()},
                       {"pricingEvidence":{"modelId":"wrong","label":"Free","prices":["Free"]*3}}, {"toolResult":False}):
            with self.assertRaises(ValueError):evidence.package_defaults(report([{**attested(),**change}]),"a"*64)

    def test_null_stream_fields_do_not_drop_valid_tool_calls(self):
        events=[{"choices":None},{"choices":[{"delta":{"tool_calls":None}}]},
                {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call1","function":{"name":"connectivity_check","arguments":""}}]}}]},
                {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":None,"arguments":'{"status":"ok"}'}}]},"finish_reason":"tool_calls"}]}]
        stream=io.BytesIO("".join("data: "+json.dumps(event)+"\n\n" for event in events).encode())
        with patch.object(verifier,"open_with_retry",return_value=stream):
            result=verifier.streamed_completion(evidence.BASE_URL+"/chat/completions",{},2)
        self.assertEqual(result["tool_calls"][0]["function"]["arguments"],'{"status":"ok"}')

    def test_one_limited_candidate_does_not_block_another_and_unknown_price_is_not_called(self):
        proof={name:{"name":name,"api":"openai-completions","provider":"opencode-free","freePricingVerified":True,
                    "pricingSource":evidence.PRICING_URL,"pricingEvidence":{"modelId":name,"label":name,"prices":["Free"]*3}} for name in ["good-free","limited-free"]}
        called=[]
        def probe(model,*_args):
            called.append(model)
            if model=="limited-free":raise urllib.error.HTTPError(evidence.BASE_URL,429,"limited",{},None)
            return {"inference":True,"streaming":True,"toolCall":True,"toolResult":True,"anonymous":True}
        with patch.object(verifier,"fetch_model_ids",return_value={"good-free","limited-free","unpriced-free"}),patch.object(verifier,"pricing_catalog",return_value=proof),patch.object(verifier,"inference_probe",side_effect=probe),patch.object(verifier,"binary_sha256",return_value="a"*64),patch.object(verifier,"verify_harness",return_value={"harnessVerified":True,"binarySha256":"a"*64}):
            result=verifier.verify_many(binary=pathlib.Path("unused-test-binary"))
        self.assertEqual(result["includedModels"],[{"provider":"opencode-free","model":"good-free"}])
        self.assertNotIn("unpriced-free",called)
        self.assertEqual(next(row for row in result["models"] if row["model"]=="limited-free")["status"],"rate-limited")

if __name__=="__main__":unittest.main()
