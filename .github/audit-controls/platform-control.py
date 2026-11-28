"""Exercise both regressions through just the public CLI/HTTP and a legacy SQLite fixture."""
import hashlib, http.client, json, socket, sqlite3, subprocess, sys, tempfile, time
from pathlib import Path

binary=Path(sys.argv[1]).resolve()
green="--green" in sys.argv[2:]
with tempfile.TemporaryDirectory(prefix="braid-platform-control-") as directory:
    root=Path(directory)
    source=root/"git"
    def run(*args): return subprocess.check_output(args, text=True).strip()
    run("git","init","-q","-b","main",str(source))
    for key,value in (("user.name","Contract"),("user.email","example@invalid"),("commit.gpgsign","false")):
        run("git","-C",str(source),"config",key,value)
    run("git","-C",str(source),"commit","--allow-empty","-qm","base")
    run("git","-C",str(source),"checkout","-qb","feature")
    (source/"body").write_text("candidate\n")
    run("git","-C",str(source),"add","body")
    run("git","-C",str(source),"commit","-qm","candidate")
    with socket.socket() as probe:
        probe.bind(("127.0.0.1",0)); port=probe.getsockname()[1]
    config=root/"platform.toml"
    run(binary,"platform","init","--config",config,"--source",source,"--default-branch","main","--name","demo","--listen",f"127.0.0.1:{port}")
    store=root/"old.db"
    old=sqlite3.connect(store)
    for file in sorted(Path("migrations").glob("000[1-5]*.sql")):
        content=file.read_bytes()
        version,name=file.stem.split("_",1)
        old.executescript(content.decode())
        old.execute("INSERT INTO schema_migrations VALUES(?,?,?,'then')",(int(version),name,hashlib.sha256(content).hexdigest()))
    old.execute("INSERT INTO local_space VALUES(1,'demo',?)",(str(source.resolve()),))
    old.execute("INSERT INTO local_items(number,repository,kind,title,body,author,created_at,updated_at) VALUES(7,'demo','issue','Imported','','human','then','then')")
    req='{"requestId":"historic","expectedRevision":1,"command":{"type":"assign","number":7,"profile":"old-profile"}}'
    orig={"requestId":"historic","state":"rejected","result":None,"error":{"code":"rejected","message":"runtime supports profile issue-codex for issue objects"}}
    old.execute("INSERT INTO control_receipts VALUES('objects','historic',?,?,'rejected',?,?,'then','then')",(hashlib.sha256(req.encode()).hexdigest(),req,json.dumps(orig),orig["error"]["message"]))
    old.commit();old.close()
    run(binary,"platform","import","--config",config,"--from",store)
    def call(path,body=None):
        c=http.client.HTTPConnection("127.0.0.1",port,timeout=5)
        c.request("GET" if body is None else "POST",path,None if body is None else json.dumps(body),{"Host":f"127.0.0.1:{port}","Content-Type":"application/json"})
        r=c.getresponse(); answer=(r.status,json.load(r));c.close();return answer
    def command(id,command,revision=None,actor="human"):return call("/api/control/commands",dict(requestId=id,actor=actor,expectedRevision=revision,command=command))
    proc=subprocess.Popen([binary,"platform","serve","--config",config],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    try:
        for i in range(100):
            try:
                assert call("/api/platform/repository")[0]==200
                break
            except ConnectionRefusedError: time.sleep(.02)
        assert call("/api/control/receipts/historic")== (200,orig)
        replay=command("historic",dict(type="assign",number=7,actor="old-profile"),1)
        print("historical assign with previous profile spelling:",replay,flush=True)
        assert replay[0]==409 and ((replay[1]==orig) if green else ("requestId was already used" in replay[1]["error"]["message"] and replay[1].get("state") is None))
        issue=command("new-issue",dict(type="createIssue",title="Task",body=""))[1]["result"]["number"]
        pr=command("new-pr",dict(type="createPR",title="Work",body="",head="feature",base="main",issue=issue,issueRevision=1))[1]["result"]["number"]
        db=sqlite3.connect(root/"platform.sqlite3")
        db.execute("""CREATE TRIGGER lose_receipt BEFORE UPDATE OF response_json ON control_receipts WHEN NEW.request_id='original' AND NEW.state!='prepared' BEGIN SELECT RAISE(ABORT,'injected receipt storage failure'); END""");db.commit()
        response=command("original",dict(type="merge",number=pr),1,"first")
        assert response[0]==422 and "injected receipt storage failure" in response[1]["error"]["message"], response
        before=call(f"/api/control/items/{pr}")[1]
        assert before["merge"]["lifecycle"]=="prepared"
        assert run("git","-C",str(source),"rev-parse","refs/heads/main")==before["merge"]["merge_commit"]
        db.execute("DROP TRIGGER lose_receipt");db.commit();db.close()
        followup=command("new-command",dict(type="merge",number=pr),1,"second")
        events=call("/api/platform/events")[1]["events"]
        actors=[r["event"]["actor_login"] for r in events if r["event"]["detail"]=="merged"]
        print("different request ID after original CAS:", followup, "merged actors:", actors,flush=True)
        if green:
            assert followup[0]==409 and "another merge request" in followup[1]["error"]["message"] and actors==[]
            assert command("original",dict(type="merge",number=pr),1,"first")[0]==200
            assert [r["event"]["actor_login"] for r in call("/api/platform/events")[1]["events"] if r["event"]["detail"]=="merged"]==["first"]
        else:
            assert followup[0]==200 and actors==["second"],(followup,actors)
    finally:
        proc.terminate();proc.wait(5)
