# 2026-06-19 re-measure: new Kimi (kimi-k2-7-code) + new Venice models since baseline.
# Same path as run_mimo_broad.py (mimo serve emulated-StructuredOutput-tool + Venice),
# but schemas are read relative to this file (portable), not a job tmp dir.
# Run: mimo serve --port 4096 &  ; then  uv run --with jsonschema python run_kimi7_sweep.py
# Override the mimo working dir / results dir with env SPIKE_SCRATCH (must contain opencode.json='{}').
import json, urllib.request, pathlib, os, time
B="http://127.0.0.1:4096"
SCHEMAS=str(pathlib.Path(__file__).resolve().parent.parent/"schemas")  # docs/spike/schemas
SC=os.environ.get("SPIKE_SCRATCH", str(pathlib.Path.home()/".mimo-spike"))
os.makedirs(SC, exist_ok=True)
if not os.path.exists(f"{SC}/opencode.json"): open(f"{SC}/opencode.json","w").write("{}")
import jsonschema
def call(path, body=None, method="GET"):
    url=f"{B}{path}?directory={SC}"; data=json.dumps(body).encode() if body is not None else None
    req=urllib.request.Request(url,data=data,method=method,headers={"content-type":"application/json"})
    try:
        with urllib.request.urlopen(req,timeout=200) as r: return r.getcode(),json.loads(r.read())
    except urllib.error.HTTPError as e:
        try: return e.code,json.loads(e.read())
        except: return e.code,{"_raw":"?"}
    except Exception as e: return 0,{"_exc":str(e)[:140]}
def load(n): return json.load(open(f"{SCHEMAS}/{n}.json"))
prompts={
 "prefilter":"Classify for skill-learning: user asked how to reverse a string in Python; assistant answered s[::-1]. Decide skip/patch_existing/create_new with a reason.",
 "cron":"A scheduled job ran: you checked a feed and found 3 new articles worth sharing. Produce delivery (notify the user with a short summary) and a technical run_note.",
}
def cat_err(e):
    el=(e or "").lower()
    if "propertyname" in el or "grammar" in el or "unimplemented" in el: return "GRAMMAR_ERR"
    if "400" in el or "invalid request" in el or "bad request" in el: return "PROVIDER_400"
    if "auth" in el or "401" in el or "unauthor" in el or "credential" in el: return "AUTH_ERR"
    return None
def one(mid,sname):
    sid=call("/session",{},"POST")[1].get("id")
    schema=load(sname)
    body={"parts":[{"type":"text","text":prompts[sname]}],"model":{"providerID":"venice","modelID":mid},"format":{"type":"json_schema","schema":schema}}
    code,resp=call(f"/session/{sid}/message",body,"POST")
    msgid=resp.get("info",{}).get("id"); st=None;finish=None;toks={};err=None
    if code>=400 or "_exc" in resp: err=json.dumps(resp)[:200]
    for _ in range(70):
        c,msgs=call(f"/session/{sid}/message")
        mine=[m for m in msgs if m.get("info",{}).get("id")==msgid] if isinstance(msgs,list) else []
        if mine:
            info=mine[0]["info"];finish=info.get("finish");st=info.get("structured");toks=info.get("tokens",{})
            ep=[p for p in mine[0].get("parts",[]) if p.get("type")=="error"]
            if ep: err=json.dumps(ep[0].get("error",ep[0]))[:200]
            if info.get("error"): err=json.dumps(info.get("error"))[:200]
            if finish is not None or err: break
        time.sleep(2)
    return finish,st,toks,err,schema
def run(mid,sname):
    for _ in range(2):
        finish,st,toks,err,schema=one(mid,sname)
        if (toks.get("input") or 0)>0 or err or finish: break
    ge=cat_err(err)
    if ge: return ge, (err or "")[:90]
    if st is None: return ("NO_STRUCTURED" if finish else "EMPTY"), (err or "")[:90]
    try: jsonschema.validate(st,schema); return "VALID",""
    except jsonschema.ValidationError as e: return "INVALID","@"+"/".join(map(str,e.absolute_path))+":"+e.message[:50]
models=[
 # anchors (known PASS — sanity that harness + Venice auth still work)
 "zai-org-glm-5","qwen3-235b-a22b-instruct-2507",
 # PRIMARY: new Kimi (TODO #1) + control (old Kimi that failed propertyNames)
 "kimi-k2-7-code","kimi-k2-6",
 # new candidates since baseline
 "deepseek-v4-flash","deepseek-v4-pro",
 "zai-org-glm-5-1","zai-org-glm-5-2","z-ai-glm-5-turbo",
 "qwen3-235b-a22b-thinking-2507","qwen3-5-397b-a17b","qwen3-6-27b","qwen3-vl-235b-a22b",
]
rows=[]
for m in models:
    r={"model":m}
    for s in ["prefilter","cron"]:
        cat,d=run(m,s); r[s]=cat; r[s+"_d"]=d
    rows.append(r)
    print(f"{m:38} prefilter={r['prefilter']:14} cron={r['cron']:14}  {r['cron_d'][:55]}",flush=True)
    json.dump(rows,open(f"{SC}/kimi7_sweep_results.json","w"),indent=1)
print("=== done ===")
