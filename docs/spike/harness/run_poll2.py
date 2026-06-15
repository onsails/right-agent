import json, urllib.request, pathlib, time
B="http://127.0.0.1:4096"; SC=str(pathlib.Path.home()/".claude/jobs/7347f6da/tmp/spike")
import jsonschema
def call(path, body=None, method="GET"):
    url=f"{B}{path}?directory={SC}"; data=json.dumps(body).encode() if body is not None else None
    req=urllib.request.Request(url, data=data, method=method, headers={"content-type":"application/json"})
    try:
        with urllib.request.urlopen(req, timeout=180) as r: return r.getcode(), json.loads(r.read())
    except urllib.error.HTTPError as e: return e.code, json.loads(e.read())
def load(n): return json.load(open(f"{SC}/schemas/{n}.json"))
prompts={
 "prefilter":"Classify for skill-learning: user asked how to reverse a string in Python; assistant answered s[::-1]. Decide skip/patch_existing/create_new with a reason.",
 "reply":"You are replying in Telegram to a user who asked 'what is 2+2'. Reply with the answer. No attachments, no skills used.",
 "cron":"A scheduled job just ran: you checked a feed and found 3 new articles worth sharing. Produce the delivery (notify the user with a short summary) and a technical run_note.",
}
def one(prov,mid,sname):
    sid=call("/session",{},"POST")[1]["id"]; schema=load(sname)
    body={"parts":[{"type":"text","text":prompts[sname]}],"model":{"providerID":prov,"modelID":mid},"format":{"type":"json_schema","schema":schema}}
    code,resp=call(f"/session/{sid}/message",body,"POST"); msgid=resp.get("info",{}).get("id")
    st=None;finish=None;toks={};err=None
    for _ in range(60):
        c,msgs=call(f"/session/{sid}/message")
        mine=[m for m in msgs if m.get("info",{}).get("id")==msgid] if isinstance(msgs,list) else []
        if mine:
            info=mine[0]["info"];finish=info.get("finish");st=info.get("structured");toks=info.get("tokens",{})
            errs=[p for p in mine[0].get("parts",[]) if p.get("type")=="error"]
            if errs: err=json.dumps(errs[0].get("error",errs[0]))[:160]
            if finish is not None or err: break
        time.sleep(2)
    return finish, st, toks, err, schema
def run(prov,mid,sname):
    rec={"model":f"{prov}/{mid}","schema":sname}
    for attempt in range(2):
        finish,st,toks,err,schema=one(prov,mid,sname)
        if (toks.get("input") or 0)>0 or err: break  # real run or explicit error; else retry empty
    rec["finish"]=finish;rec["in"]=toks.get("input");rec["out"]=toks.get("output");rec["err"]=err
    if st is None: rec["valid"]="NO_STRUCTURED"
    else:
        try: jsonschema.validate(st,schema); rec["valid"]="VALID"
        except jsonschema.ValidationError as e: rec["valid"]="INVALID: "+e.message[:90]
    rec["structured"]=st; return rec
plan=[("mimo","mimo-auto","prefilter"),("mimo","mimo-auto","reply"),("mimo","mimo-auto","cron"),("openai","gpt-5.4-mini","prefilter")]
rows=[run(*p) for p in plan]
for r in rows: print(json.dumps({k:r.get(k) for k in ("model","schema","finish","valid","in","out","err")}))
json.dump(rows, open(f"{SC}/poll2_results.json","w"), indent=1)
