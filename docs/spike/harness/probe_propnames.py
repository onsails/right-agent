import json, urllib.request, pathlib, time
B="http://127.0.0.1:4096"; SC=str(pathlib.Path.home()/".claude/jobs/7347f6da/tmp/spike")
def call(path, body=None, method="GET"):
    url=f"{B}{path}?directory={SC}"; data=json.dumps(body).encode() if body is not None else None
    req=urllib.request.Request(url,data=data,method=method,headers={"content-type":"application/json"})
    try:
        with urllib.request.urlopen(req,timeout=120) as r: return r.getcode(),json.loads(r.read())
    except urllib.error.HTTPError as e:
        try: return e.code,json.loads(e.read())
        except: return e.code,{}
    except Exception as e: return 0,{"_exc":str(e)[:120]}
MIN={"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}  # flat, no propertyNames anywhere
def probe(mid, schema, label):
    sid=call("/session",{},"POST")[1].get("id")
    body={"parts":[{"type":"text","text":"Reply with a JSON object whose 'answer' is the string 'hi'."}],
          "model":{"providerID":"venice","modelID":mid},"format":{"type":"json_schema","schema":schema}}
    code,resp=call(f"/session/{sid}/message",body,"POST"); msgid=resp.get("info",{}).get("id")
    st=None;finish=None;err=None
    for _ in range(40):
        c,msgs=call(f"/session/{sid}/message")
        mine=[m for m in msgs if m.get("info",{}).get("id")==msgid] if isinstance(msgs,list) else []
        if mine:
            info=mine[0]["info"];finish=info.get("finish");st=info.get("structured")
            ep=[p for p in mine[0].get("parts",[]) if p.get("type")=="error"]
            if ep: err=json.dumps(ep[0].get("error",ep[0]))[:220]
            if info.get("error"): err=json.dumps(info.get("error"))[:220]
            if finish or err: break
        time.sleep(2)
    print(f"[{label}] {mid}: finish={finish} structured={st} err={err}")
probe("kimi-k2-6", MIN, "Kimi + MINIMAL flat schema (no propertyNames)")
probe("zai-org-glm-5", MIN, "GLM-5 + MINIMAL (control)")
probe("kimi-k2-6", json.load(open(f"{SC}/schemas/prefilter.json")), "Kimi + our prefilter schema (baseline)")
