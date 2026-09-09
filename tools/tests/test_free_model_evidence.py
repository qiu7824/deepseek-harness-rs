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
    def test_client_restriction_is_explicit_and_never_enters_package_evidence(self):
        payload = {"error": {"type": "MissingSessionID", "message": "OpenCode's free tier can only be used in OpenCode"}}
        def denied(*_args):
            raise urllib.error.HTTPError(evidence.BASE_URL, 400, "Bad Request", {}, io.BytesIO(json.dumps(payload).encode()))
        proof = {"blocked-free": {"name": "Blocked", "api": "openai-completions", "provider": "opencode-free", "freePricingVerified": True}}
        with patch.object(verifier, "fetch_model_ids", return_value={"blocked-free"}), patch.object(verifier, "pricing_catalog", return_value=proof), patch.object(verifier, "inference_probe", side_effect=denied), patch.object(verifier, "binary_sha256", return_value="a" * 64), patch.object(verifier, "verify_harness") as harness:
            result = verifier.verify_many(binary=pathlib.Path("fixture"))
        harness.assert_not_called()
        self.assertEqual(result["models"][0]["reason"], verifier.CLIENT_RESTRICTION_REASON)
        self.assertEqual(result["models"][0]["failureCode"], "PROVIDER_CLIENT_RESTRICTED")
        self.assertFalse(result["models"][0]["available"])
        self.assertEqual(result["includedModels"], [])
        with self.assertRaises(ValueError): evidence.validated_models(result)

    def test_anonymous_request_keeps_its_real_client_identity(self):
        captured=[]
        def response(request, timeout):
            captured.append(request)
            return io.BytesIO(b'data: {"choices":[{"delta":{"content":"OK"},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n')
        with patch.object(verifier, "open_with_retry", side_effect=response):
            verifier.streamed_completion(evidence.BASE_URL + "/chat/completions", {"model": "fixture"}, 2)
        headers={key.lower():value for key,value in captured[0].header_items()}
        self.assertEqual(headers["user-agent"], "deepseek-harness-rs-release-verifier")
        self.assertRegex(headers["x-opencode-session"], r"^dsh-verifier-[0-9a-f]{32}$")
        self.assertFalse(any("client" in key or "authorization" in key or "cookie" in key for key in headers))

    def test_tool_round_trip_keeps_one_opaque_routing_key(self):
        routing=[]
        def completion(_endpoint,_body,_timeout,session):
            routing.append(session)
            if len(routing)==1:
                return {"role":"assistant","content":"","tool_calls":[{"id":"call-1","type":"function","function":{"name":"connectivity_check","arguments":'{"status":"ok"}'}}]}
            return {"role":"assistant","content":"OK"}
        with patch.object(verifier,"streamed_completion",side_effect=completion):
            value=verifier.inference_probe("fixture",evidence.CATALOG_URL)
        self.assertTrue(value["toolResult"])
        self.assertEqual(len(routing),2)
        self.assertEqual(routing[0],routing[1])

    def test_catalog_failure_keeps_a_structured_failure_report(self):
        with tempfile.TemporaryDirectory() as directory:
            path=pathlib.Path(directory)/"failed.json"
            with patch.object(verifier,"fetch_model_ids",side_effect=ValueError("catalog unavailable")):
                with self.assertRaises(ValueError): verifier.verify_many(report_path=path)
            failed=json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(failed["includedModels"],[])
            self.assertEqual(failed["verificationError"]["reason"],"catalog unavailable")

    def test_single_model_cli_keeps_strict_failure_and_diagnostic_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            path=pathlib.Path(directory)/"failure.json"
            error=urllib.error.HTTPError(evidence.BASE_URL,400,"Bad Request",{},io.BytesIO(json.dumps({"type":"MissingSessionID","message":"OpenCode's free tier can only be used in OpenCode"}).encode()))
            with patch.object(verifier,"verify",side_effect=error), patch.object(sys,"argv",["verify","--report",str(path)]):
                with self.assertRaisesRegex(SystemExit,verifier.CLIENT_RESTRICTION_REASON):verifier.main()
            failed=json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(failed["models"][0]["failureCode"],verifier.CLIENT_RESTRICTION_CODE)
            self.assertEqual(failed["includedModels"],[])
            with self.assertRaises(ValueError):evidence.validated_models(failed)

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
