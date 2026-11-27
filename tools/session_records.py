"""Select user-facing events from a single registered development session.

Keep native message shapes and ordering. Opaque tool payloads can contain private
instructions, credentials or learner data, so retain their tool identity and status.
"""
import re

REMINDER = re.compile(r"<system-reminder\b[^>]*>.*?(?:</system-reminder>|$)", re.S | re.I)


def text(value):
    return REMINDER.sub("", value) if isinstance(value, str) else ""


def fields(record, names):
    return {k: record[k] for k in names if isinstance(record.get(k), (str, bool, int))}


def claude(record):
    kind = record.get("type")
    message = record.get("message")
    if (kind not in ("user", "assistant") or not isinstance(message, dict)
            or message.get("role") != kind or record.get("isMeta") or record.get("isCompactSummary")
            or message.get("channel") in ("analysis", "internal")):
        return None
    content = message.get("content")
    if isinstance(content, list):
        blocks = []
        for b in content:
            if not isinstance(b, dict):
                continue
            if b.get("type") == "text" and text(b.get("text")).strip():
                blocks.append({"type": "text", "text": text(b["text"])})
            elif b.get("type") == "tool_result":
                blocks.append(fields(b, ("type", "tool_use_id", "is_error")))
            elif b.get("type") == "tool_use":
                block = fields(b, ("type", "id", "name"))
                args = b.get("input") if isinstance(b.get("input"), dict) else {}
                allowed = "text" if str(block.get("name", "")).endswith("post_message") else "description"
                block["input"] = {allowed: text(args[allowed])} if isinstance(args.get(allowed), str) else {}
                blocks.append(block)
        content = blocks
    else:
        content = text(content)
    if not content:
        return None
    clean = fields(record, ("type", "uuid", "parentUuid", "sessionId", "timestamp", "isSidechain"))
    clean["message"] = {"role": kind, "content": content}
    return clean


def codex(record):
    obj = record.get("payload")
    if not isinstance(obj, dict):
        return None
    kind = obj.get("type")
    if record.get("type") == "event_msg" and kind in ("user_message", "agent_message"):
        if obj.get("phase") not in (None, "commentary", "final_answer") or not text(obj.get("message")).strip():
            return None
        payload = fields(obj, ("type", "phase", "turn_id"))
        payload["message"] = text(obj["message"])
    elif record.get("type") == "event_msg" and kind == "item_completed":
        item = obj.get("item")
        if not isinstance(item, dict) or item.get("type") not in ("UserMessage", "AgentMessage", "Plan"):
            return None
        if item.get("phase") not in (None, "commentary", "final_answer") or item.get("channel") in ("analysis", "internal"):
            return None
        safe = fields(item, ("type", "id", "phase"))
        if item["type"] == "Plan":
            safe["text"] = text(item.get("text"))
        else:
            safe["content"] = [{"type": b.get("type"), "text": text(b["text"])}
                               for b in item.get("content", []) if isinstance(b, dict)
                               and b.get("type") in ("text", "input_text", "output_text", "Text")
                               and isinstance(b.get("text"), str)]
        payload = {"type": kind, "item": safe}
    elif record.get("type") == "response_item" and kind in (
            "function_call", "custom_tool_call", "function_call_output", "custom_tool_call_output"):
        payload = fields(obj, ("type", "id", "name", "call_id", "status"))
    else:
        # User-role response_item copies can contain injected instructions. The
        # event_msg record is the actual user-facing copy.
        return None
    return {**fields(record, ("timestamp", "type")), "payload": payload}
