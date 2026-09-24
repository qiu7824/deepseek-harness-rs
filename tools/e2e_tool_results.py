"""Read native V4 and migrated legacy tool results without losing failures."""


def tool_result_blocks(events):
    results = []
    for event in events:
        if event.get("type") != "tool/result":
            continue
        message = event["data"]["message"]
        if message.get("role") == "tool":
            call_id = message.get("toolCallId")
            source = message.get("source", {})
            if not call_id or source.get("kind") != "tool" or source.get("callId") != call_id:
                raise ValueError("native tool result has inconsistent call ownership")
            blocks = [{"type": "tool-result", "toolCallId": call_id,
                       "isError": message.get("isError", False), "content": message["content"]}]
        else:
            blocks = [block for block in message.get("content", []) if block.get("type") == "tool-result"]
            if not blocks:
                raise ValueError("tool result event has no result payload")
        for block in blocks:
            if not isinstance(block.get("content"), list) or not block.get("toolCallId"):
                raise ValueError("malformed tool result payload")
            if "isError" in block and not isinstance(block["isError"], bool):
                raise ValueError("tool result error flag must be boolean")
            results.append({**block, "isError": block.get("isError", False)})
    return results
