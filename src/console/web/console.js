'use strict';
const content=document.getElementById('content'), notice=document.getElementById('notice');
let generation=0,controlPending=false,activeHash=location.hash||'#overview';
const labels={
  overview:'概览',traces:'Trace 时间轴',logs:'日志与会话',metrics:'指标',objects:'协作空间'
};
function el(tag,attrs={},...children){
  const n=document.createElement(tag);
  for(const [k,v] of Object.entries(attrs)){
    if(k==='class')n.className=v;
    else if(k.startsWith('on'))n.addEventListener(k.slice(2),v);
    else if(v!==undefined&&v!==null)n.setAttribute(k,v);
  }
  for(const c of children.flat(Infinity)){
    if(c!==null&&c!==undefined)n.append(c instanceof Node?c:document.createTextNode(String(c)));
  }
  return n;
}
function json(v){
  return JSON.stringify(v,null,2)??'未记录';
}
function val(v){
  if(!v||typeof v!=='object')return v;
  if('stringValue'in v)return v.stringValue;
  if('intValue'in v)return v.intValue;
  if('doubleValue'in v)return v.doubleValue;
  if('boolValue'in v)return v.boolValue;
  if(v.arrayValue)return(v.arrayValue.values||[]).map(val);
  if(v.kvlistValue)return Object.fromEntries((v.kvlistValue.values||[]).map(a=>[a.key,val(a.value)]));
  return v;
}
function attrs(v){
  return Object.fromEntries((v||[]).map(a=>[a.key,val(a.value)]));
}
function text(v){
  return typeof v==='string'?v:json(val(v));
}
function stamp(v){
  if(!v)return'未收到';
  const d=new Date(Number(v));
  return Number.isNaN(d.valueOf())?String(v):d.toLocaleString();
}
function nanos(v){
  if(!v||String(v)==='0')return'事件时间未记录';
  try{
    return stamp(Number(BigInt(v)/1000000n));
  }
  catch{
    return String(v);
  }
}
function badge(v,bad=false){
  return el('span',{
    class:'badge'+(bad?' bad':'')
  },v);
}
function raw(v,label='完整记录'){
  return el('details',{},el('summary',{},label),el('pre',{},json(v)));
}
function panel(title,...children){
  return el('section',{
    class:'panel'
  },title?el('h2',{},title):null,...children);
}
function empty(message='尚未收到记录'){
  return el('div',{
    class:'empty'
  },message);
}
function showError(e){
  notice.replaceChildren(el('div',{
    class:'notice error'
  },e.message||String(e)));
}
async function api(path,body){
  const r=await fetch(path,body===undefined?{signal:AbortSignal.timeout(30000)}
  :{
    method:'POST',signal:AbortSignal.timeout(30000),headers:{
      'Content-Type':'application/json'
    },body:JSON.stringify(body)
  });
  const t=await r.text();
  let v;
  try{
    v=JSON.parse(t);
  }
  catch{
    v=t;
  }
  if(!r.ok){const error=new Error(`HTTP ${r.status} ${r.statusText}\n${text(v)}`);error.status=r.status;error.body=v;throw error;}
  return v;
}
function query(v){
  return new URLSearchParams(Object.entries(v).filter(([,x])=>x!==''&&x!==undefined&&x!==null)).toString();
}
function button(label,action,kind=''){
  return el('button',{
    type:'button',class:kind,onclick:async e=>{
      const target=e.currentTarget;target.disabled=true;try{
        await action();
      }
      catch(err){
        showError(err);
      }
      finally{
        if(target.isConnected)target.disabled=false;
      }
    }
  },label);
}
function field(name,value='',type='text'){
  const input=el(type==='textarea'?'textarea':'input',type==='textarea'?{}
  :{
    type,value
  });
  if(type==='textarea')input.value=value;
  return{
    input,node:el('label',{},name,input)
  };
}
function table(headers,rows){
  return el('div',{
    class:'table-wrap'
  },el('table',{},el('thead',{},el('tr',{},headers.map(h=>el('th',{},h)))),el('tbody',{},rows.map(r=>el('tr',{},r.map(c=>el('td',{},c)))))));
}
async function overview(token){
  const [resources,runtime]=await Promise.all([api('/api/resources'),api('/api/runtime').catch(e=>({
    unavailable:e.message
  }))]);
  if(token!==generation)return;
  const cards=el('div',{
    class:'grid'
  },...[[ '已接收记录',resources.records??0],['接收批次',resources.batches??0],['资源来源',(resources.resources||[]).length],['接收错误',resources.receive_errors??0],['重复批次',resources.duplicate_batches??0],['最近接收',stamp(resources.last_received)]].map(([title,value])=>panel(null,el('div',{
    class:'muted'
  },title),el('div',{
    class:'stat'
  },value))));
  content.replaceChildren(cards,panel('Braid 权威状态',runtime.unavailable?el('div',{
    class:'notice error'
  },runtime.unavailable):runtimePanel(runtime)),panel('采集来源',(resources.resources||[]).length?table(['实例','服务','首次 / 最近接收','记录数'],resources.resources.map(r=>[r.instance||'未标识',r.service||'未标识',`${stamp(r.first_received)} / ${stamp(r.last_received)}`,r.records])):empty('Collector 正在等待 OTLP 数据。请检查 Braid 的 exporter 目标。')),panel('数据边界',el('p',{
    class:'body'
  },'接收时间表示 Collector 收到数据的时刻，不是运行完成时间。采样、网络错误或 exporter 停止都可能造成未收到的内容。没有上游序列与交付回执时，采集缺口不可量化；Trace 缺少父 Span、Transcript 缺少消息均保留为未确认。'),raw(resources,'完整采集状态')));
  if(resources.receive_errors){
    const errors=await api('/api/receive-errors?limit=100');
    if(token===generation)content.append(panel('接收错误（最近已加载，完整原因保留）',table(['接收时刻','信号 / HTTP','原因','原始数据'],errors.errors.map(e=>[stamp(e.received_at),`${e.signal} / ${e.status}`,el('pre',{},e.reason),el('a',{
      href:'/api/receive-errors/'+encodeURIComponent(e.id)
    },'下载失败请求正文')]))));
  }
}
function runtimePanel(runtime){
  const transport=el('input',{
    type:'checkbox'
  }),receipt=el('div');
  async function command(operation){
    if(controlPending)return;
    controlPending=true;
    const id=requestId();
    receipt.replaceChildren(el('p',{
      class:'meta'
    },`请求 ${id} · 等待权威端受理`));
    try{
      const result=await api('/api/runtime/commands',{
        request_id:id,operation,expected_generation:runtime.generation,transport_only:transport.checked
      });
      receipt.replaceChildren(el('div',{
        class:'row'
      },badge(result.state||'未报告'),el('code',{},result.request_id||id)),raw(result,'控制受理回执'),el('p',{
        class:'body'
      },'受理不等于生命周期完成。点击刷新，核对新的运行状态与 Generation。'));
    }
    catch(e){
      showError(e);
      receipt.replaceChildren(el('p',{
        class:'body'
      },'请求是否受理尚未确认。请查询回执，不要根据网络错误重复操作。'),el('code',{},id),button('查询回执',async()=>receipt.replaceChildren(raw(await api('/api/runtime/receipts/'+encodeURIComponent(id)),'完整运行控制回执'))));
    }
    finally{controlPending=false;}
  }
  return el('div',{},el('div',{
    class:'row'
  },badge(runtime.state,runtime.state==='blocked'),el('strong',{},runtime.instance||'未标识实例'),el('span',{
    class:'meta'
  },runtime.repository||'')),el('p',{
    class:'meta'
  },`Generation ${runtime.generation||'无'} · Provider ${runtime.provider_enabled?'启用':'禁用'} · Ready ${runtime.health?.ready===true?'是':'否 / 未报告'}`),el('p',{
    class:'meta'
  },`OTLP ${runtime.telemetry?.endpoint||'未配置'} · 采样 ${runtime.telemetry?.sample_ratio??'未报告'} · Incident ${runtime.telemetry?.incident_mode?'开启':'关闭'}`),runtime.evidence?el('p',{
    class:'meta'
  },`已发原生内容 ${runtime.evidence.emitted_payloads} · 未采样 ${runtime.evidence.unsampled_payloads} · 未关联 ${runtime.evidence.unassigned_payloads} · 已注册 Session ${runtime.evidence.registered_sessions}（本 Generation）`):null,runtime.error?el('div',{
    class:'notice error'
  },text(runtime.error)):null,runtime.health?.last_error?el('div',{
    class:'notice error'
  },text(runtime.health.last_error)):null,el('label',{
    class:'row'
  },transport,'仅运行传输与调度，不启动 Provider / 模型'),el('div',{
    class:'actions'
  },button('启动 Braid',()=>command('start'),'primary'),button('停止 Braid',async()=>{
    if(await confirmAction('停止本实例 Braid runtime？Collector 保持接收与查询服务。'))await command('stop');
  },'danger')),receipt,raw(runtime,'完整权威运行状态'));
}
async function telemetry(page,token){
  const signal=page==='traces'?'traces':page==='metrics'?'metrics':'logs';
  const instance=field('实例'),session=field('Provider Session'),turn=field('Turn ID'),trace=field('Trace ID');
  const selected=new URLSearchParams(location.hash.split('?')[1]||'');
  instance.input.value=selected.get('instance')||'';
  session.input.value=selected.get('session')||'';
  turn.input.value=selected.get('turn')||'';
  trace.input.value=selected.get('trace')||'';
  const records=[],target=el('div');
  let after=0,loadSequence=0;
  const more=button('加载更多',()=>load(false));
  const status=el('div',{
    class:'meta'
  });
  async function load(reset){
    const sequence=++loadSequence;
    if(reset){
      records.length=0;
      after=0;
    }
    const result=await api('/api/records?'+query({
      signal,instance:instance.input.value,session:session.input.value,turn:turn.input.value,trace:trace.input.value,after,limit:100
    }));
    if(token!==generation||sequence!==loadSequence)return;
    records.push(...result.records);
    after=result.next_after;
    status.textContent=`已加载 ${records.length} 条 · 按接收顺序 · 每页最多 100 条`;
    more.hidden=result.records.length<100;
    target.replaceChildren(...(page==='traces'?renderTraces(records):page==='metrics'?renderMetrics(records):renderLogs(records)));
    if(!records.length)target.append(empty('当前筛选下尚未收到记录。'));
  }
  const filters=el('form',{
    class:'toolbar',onsubmit:e=>{
      e.preventDefault();load(true).catch(showError);
    }
  },instance.node,session.node,turn.node,trace.node,el('button',{
    type:'submit',class:'primary'
  },'筛选'));
  content.replaceChildren(filters,el('p',{class:'meta'},page==='metrics'?'指标按原始批次与数据点展示，不跨批次求和或去重；重复接收的批次会明确标识。':'只展示已收到记录；Provider 原生事件、流式片段与未关联内容保持原样，不拼造完整会话。'),status,target,more);
  await load(true);
}
function recordMeta(r){
  return el('div',{
    class:'meta'
  },`${r.duplicate_of?'重复接收（原批次 #'+r.duplicate_of+'） · ':''}接收 ${stamp(r.received_at)} · ${r.instance||'实例未标识'} / ${r.service||'服务未标识'}`,el('br'),`Session ${r.session||'未记录'} · Trace ${r.trace_id||'未记录'} · 记录 #${r.id}`,el('br'),r.trace_id?el('a',{
    href:'#traces?trace='+encodeURIComponent(r.trace_id)
  },'查看关联 Trace'):null,r.trace_id?' · ':null,r.session?el('a',{
    href:'#logs?session='+encodeURIComponent(r.session)
  },'阅读此 Session'):null,r.session?' · ':null,el('a',{
    href:'/api/batches/'+encodeURIComponent(r.batch_id)
  },'下载原始 OTLP 批次'));
}
function renderTraces(records){
  const groups=new Map();
  for(const r of records){
    const key=r.trace_id||`未标识 #${r.id}`;
    if(!groups.has(key))groups.set(key,[]);
    groups.get(key).push(r);
  }
  return [...groups].map(([id,rows])=>{
    const spans=rows.map(r=>r.data),times=spans.flatMap(s=>[s.startTimeUnixNano,s.endTimeUnixNano]).filter(v=>v&&/^\d+$/.test(v)).map(BigInt),begin=times.length?times.reduce((a,b)=>a<b?a:b):0n,end=times.length?times.reduce((a,b)=>a>b?a:b):0n;const spanIds=new Set(spans.map(s=>s.spanId));return panel(`Trace ${id}`,el('div',{
      class:'meta'
    },`${rows.length} 个已加载 Span · ${nanos(begin.toString())} · 仅展示已收到区间`),el('a',{
      href:'#logs?trace='+encodeURIComponent(id)
    },'阅读关联日志与 Provider Transcript'),...rows.map(r=>{
      const s=r.data,start=/^\d+$/.test(s.startTimeUnixNano||'')?BigInt(s.startTimeUnixNano):begin,finish=/^\d+$/.test(s.endTimeUnixNano||'')?BigInt(s.endTimeUnixNano):start,duration=Number(finish-start)/1e6,range=Number(end-begin)||1;const bar=el('span',{
        class:'timeline-bar'+(s.status?.code===2?' error':'')
      });bar.style.left=`${Math.max(0,Math.min(100,Number(start-begin)/range*100))}%`;bar.style.width=`${Math.max(0,Math.min(100,Number(finish-start)/range*100))}%`;return el('div',{
        class:'record'
      },el('div',{
        class:'timeline-row'
      },el('div',{},el('strong',{},s.name||'未命名 Span'),el('div',{
        class:'meta'
      },`${duration.toFixed(2)} ms · ${s.spanId||r.span_id||''}`)),el('div',{
        class:'timeline-track'
      },bar)),s.parentSpanId&&!spanIds.has(s.parentSpanId)?el('p',{
        class:'meta'
      },`父 Span ${s.parentSpanId} 尚未在已加载数据中找到`):null,recordMeta(r),(s.events||[]).length?el('details',{},el('summary',{},`关联事件 · ${s.events.length}`),...(s.events||[]).map(e=>el('div',{
        class:'record'
      },el('strong',{},e.name),el('div',{
        class:'meta'
      },nanos(e.timeUnixNano)),raw(e,'完整事件')))):null,raw(r));
    }));
  });
}
function transcript(payload){
  let p=payload;
  if(typeof p==='string'){
    try{
      p=JSON.parse(p);
    }
    catch{
      return el('pre',{},p);
    }
  }
  if(!p||typeof p!=='object')return el('div',{
    class:'body'
  },text(p));
  const message=p.message||p.item||p.params?.item||p.params?.message||p;
  const blocks=Array.isArray(message.content)?message.content:[];
  return el('div',{},el('div',{
    class:'row'
  },badge(p.type||p.method||'原生事件'),message.role?badge(message.role):null),...blocks.map(b=>{
    if(typeof b==='string')return el('div',{
      class:'body'
    },b);if(['text','input_text','output_text'].includes(b.type))return el('div',{
      class:'body'
    },b.text||'');if(['thinking','reasoning'].includes(b.type))return raw(b,'思考内容');if(['toolCall','function_call'].includes(b.type))return raw(b,`工具调用 · ${b.name||''} · ${b.id||b.call_id||''}`);return raw(b,`原生内容 · ${b.type||'未知'}`);
  }),message.output!==undefined?el('pre',{},text(message.output)):null,message.errorMessage?el('div',{
    class:'notice error'
  },text(message.errorMessage)):null,!blocks.length?raw(p,'完整原生消息 / 流式事件'):null);
}
function renderLogs(records){
  return [panel('日志 / Provider Transcript',...records.map(r=>{
    const d=r.data,a=attrs(d.attributes),body=val(d.body),name=a['event.name']||a['name']||a['braid.event']||d.eventName||d.severityText||'日志',native=a['provider.transcript'];return el('article',{
      class:'record'
    },el('header',{},badge(name,d.severityNumber>=17),el('span',{
      class:'meta'
    },nanos(d.timeUnixNano&&d.timeUnixNano!=='0'?d.timeUnixNano:d.observedTimeUnixNano))),recordMeta(r),el('p',{
      class:'meta'
    },`Turn ${r.turn_id||a['braid.turn.id']||a['provider.turn.id']||'未关联'}${a['observation.correlated']===false?' · 未关联至权威会话':''}`),native!==undefined?transcript(native):el('div',{
      class:'body'
    },text(body)),raw(a,'记录属性（含完整原生 Transcript）'),raw(r));
  }))];
}
function renderMetrics(records){
  return records.map(r=>{
    const d=r.data,kind=['gauge','sum','histogram','exponentialHistogram','summary'].find(k=>d[k]),points=d[kind]?.dataPoints||[];return panel(d.name||'未命名指标',el('div',{
      class:'meta'
    },`${kind||'未知类型'} · ${d.unit||'无单位'} · ${d.description||''}`),recordMeta(r),points.length?table(['采样时刻','数值 / 分布','属性'],points.map(p=>[nanos(p.timeUnixNano),el('span',{
      class:'metric-point'
    },text(p.asDouble??p.asInt??{
      count:p.count,sum:p.sum,min:p.min,max:p.max,buckets:p.bucketCounts,explicitBounds:p.explicitBounds,quantiles:p.quantileValues
    })),text(attrs(p.attributes))])):empty('未记录数据点'),raw(r));
  });
}
async function route(){
  activeHash=location.hash||'#overview';
  const page=location.hash.slice(1).split('?')[0]||'overview',token=++generation;
  notice.replaceChildren();
  document.getElementById('page-title').textContent=labels[page]||'概览';
  document.querySelectorAll('nav a').forEach(a=>{
    if(a.hash==='#'+page)a.setAttribute('aria-current','page');else a.removeAttribute('aria-current');
  });
  content.setAttribute('aria-busy','true');
  content.replaceChildren(empty('正在读取…'));
  try{
    if(page==='overview'||!labels[page])await overview(token);
    else if(page==='objects')await objects(token);
    else await telemetry(page,token);
  }
  catch(e){
    if(token===generation)showError(e);
  }
  finally{
    if(token===generation)content.setAttribute('aria-busy','false');
  }
}
document.getElementById('refresh').addEventListener('click',guardedRefresh);
window.addEventListener('hashchange',async()=>{const target=location.hash;if(controlPending||hasDrafts()){history.replaceState(null,'',activeHash);if(controlPending){showError(new Error('控制请求尚未取得回执，暂时不能切换页面。'));return;}if(!await confirmAction('离开此页将丢弃尚未提交的草稿，继续吗？'))return;history.replaceState(null,'',target);}route();});
function hasDrafts(){return [...content.querySelectorAll('[data-draft]')].some(n=>n.value!==n.dataset.initialValue);}
function confirmAction(message){return new Promise(resolve=>{const overlay=el('div',{class:'confirm-overlay',role:'alertdialog','aria-modal':'true','aria-label':'确认操作'},el('section',{class:'panel'},el('h2',{},'确认操作'),el('p',{class:'body'},message),el('div',{class:'actions'},button('取消',async()=>finish(false)),button('继续',async()=>finish(true),'primary'))));function finish(value){overlay.remove();resolve(value);}overlay.addEventListener('keydown',event=>{if(event.key==='Escape'){event.preventDefault();finish(false);}if(event.key==='Tab'){event.preventDefault();const buttons=[...overlay.querySelectorAll('button')];const index=buttons.indexOf(document.activeElement);buttons[(index+(event.shiftKey?-1:1)+buttons.length)%buttons.length].focus();}});document.body.append(overlay);overlay.querySelector('button').focus();});}
async function guardedRefresh(){if(controlPending){showError(new Error('控制请求尚未取得回执，请等待或查询回执。'));return;}if(hasDrafts()&&!await confirmAction('刷新将丢弃尚未提交的正文、评论与创建草稿，继续吗？'))return;route();}
document.addEventListener('click',async event=>{const anchor=event.target.closest('a[href^="#"]');if(!anchor||anchor.hash===location.hash)return;if(controlPending){event.preventDefault();showError(new Error('控制请求尚未取得回执，暂时不能切换页面。'));return;}if(hasDrafts()){event.preventDefault();if(await confirmAction('离开此页将丢弃尚未提交的正文、评论与创建草稿，继续吗？')){content.querySelectorAll('[data-draft]').forEach(n=>n.dataset.initialValue=n.value);location.hash=anchor.hash;}}});
window.addEventListener('beforeunload',event=>{if(controlPending||hasDrafts()){event.preventDefault();event.returnValue='';}});
function requestId(){
  return crypto.randomUUID();
}
async function objects(token){
  const snapshot=await api('/api/control/snapshot');
  if(token!==generation)return;
  const list=el('div'),detail=el('div'),receipt=el('div');
  const createTitle=field('标题'),createBody=field('正文','','textarea'),kind=el('select',{},el('option',{
    value:'issue'
  },'Issue'),el('option',{
    value:'pr'
  },'Pull Request')),head=field('Head 分支'),base=field('Base 分支'),issue=field('关联 Issue 编号','','number');
  let current=null,profiles=snapshot.supportedProfiles||{};
  const reply=field('回复评论编号（可选）','','number');
  let lastRequest=null;
  function renderReceipt(value){
    receipt.replaceChildren(panel('控制回执',el('div',{
      class:'row'
    },badge(value.state||'未报告'),el('code',{},value.requestId||value.request_id||'')),el('p',{
      class:'body'
    },'applied 表示协作对象接受本次请求。它不表示 Agent 已开始、结束或业务已完成。'),value.error?el('div',{class:'notice error'},text(value.error)):null,raw(value,'完整回执')));
  }
  async function execute(command,revision){
    if(controlPending)return;
    controlPending=true;
    const id=requestId();
    lastRequest=id;
    receipt.replaceChildren(panel('控制请求',el('p',{
      class:'meta'
    },`请求 ${id}`),el('p',{},'正在等待权威端回执…')));
    try{
      const result=await api('/api/control/commands',{
        requestId:id,expectedRevision:revision,command
      });
      renderReceipt(result);
      if(command.type==='createIssue'||command.type==='createPR'){for(const f of [createTitle,createBody,head,base,issue]){f.input.value='';f.input.dataset.initialValue='';}}
      await reload();
      if(current)await select(current.item.number);
    }
    catch(e){
      showError(e);
      if(e.body?.requestId&&e.body?.state){renderReceipt(e.body);return;}
      receipt.replaceChildren(panel('尚未确认本次请求',el('p',{
        class:'body'
      },'请求失败或连接中断不等于写入未发生。请按请求 ID 查询回执，核对对象当前版本。'),el('code',{},id),button('查询此请求回执',async()=>renderReceipt(await api('/api/control/receipts/'+encodeURIComponent(id))))));
    }
    finally{controlPending=false;}
  }
  async function reload(){
    const s=await api('/api/control/snapshot');
    profiles=s.supportedProfiles||{};
    if(token!==generation)return;
    list.replaceChildren(panel('Issue / Pull Request',s.items.length?table(['对象','状态','指派','修订'],s.items.map(v=>{
      const i=v.item;return[button(`#${i.number} ${i.title}`,()=>select(i.number),'link-button'),badge(i.kind+' · '+i.state),i.desired_profile||'未指派',i.revision];
    })):empty('协作空间为空。创建 Issue 开始讨论。')),panel('运行与调度事实',el('p',{
      class:'meta'
    },`来源 ${s.source} · 观察 ${s.observedAt||'未记录'}`),raw({
      sessions:s.sessions,turns:s.turns,queue:s.queue,assignments:s.assignments,worktrees:s.worktrees
    },'Provider Session、Turn、队列、Assignment 与 Worktree')));
  }
  async function select(number){
    const v=await api('/api/control/items/'+number);
    if(token!==generation)return;
    current=v;
    const i=v.item,title=field('标题',i.title),body=field('正文',i.body,'textarea'),comment=field('新评论','','textarea'),profile=field('Profile',i.desired_profile||(i.kind==='issue'?profiles.issue:profiles.pr)||''),linked=field('关联 PR 编号','','number');
    const merged=v.merge?.lifecycle==='applied';
    const heading=el('div',{
      class:'row'
    },el('h2',{},`#${i.number} ${i.title}`),badge(i.kind+' · '+(merged?'merged':i.state)));
    const actions=el('div',{
      class:'actions'
    },button('保存标题与正文',()=>execute({
      type:'edit',number:i.number,title:title.input.value,body:body.input.value
    },i.revision),'primary'),button('指派',()=>execute({
      type:'assign',number:i.number,profile:profile.input.value
    },i.revision)),button('解除指派',()=>execute({
      type:'unassign',number:i.number
    },i.revision)),merged?el('span',{class:'badge'},'已合并，不能重新打开'):button(i.state==='closed'?'重新打开':'关闭',()=>execute({
      type:i.state==='closed'?'reopen':'close',number:i.number
    },i.revision),'danger'));
    const controls=panel('编辑与控制',el('div',{
      class:'form-grid'
    },title.node,body.node,profile.node),actions);
    const discussion=panel('讨论',...(v.comments||[]).map(c=>el('article',{
      class:'record'
    },el('div',{
      class:'meta'
    },`#${c.comment_id} · ${c.author} · ${c.created_at}${c.reply_to?' · 回复 #'+c.reply_to:''}`),el('div',{
      class:'body'
    },c.body),button('回复',async()=>{
      reply.input.value=c.comment_id;comment.input.focus();
    },'link-button'))),comment.node,reply.node,el('div',{
      class:'actions'
    },button('发布评论',()=>execute({
      type:'comment',number:i.number,body:comment.input.value,...(reply.input.value?{
        replyTo:Number(reply.input.value)
      }
      :{})
    },i.revision),'primary')));
    let candidate=null;
    if(i.kind==='pull_request'||i.kind==='pr'){
      const worktree=field('权威 Worktree ID'),diff=el('div'),candidateKind=el('select',{},el('option',{value:'source'},'配置 Source 的当前已提交 Checkout'),el('option',{value:'worktree'},'权威 Worktree'));worktree.node.hidden=true;candidateKind.addEventListener('change',()=>worktree.node.hidden=candidateKind.value!=='worktree');
      candidate=panel('Git 候选',badge(i.published?'候选已发布':'候选未发布'),el('p',{
        class:'meta'
      },`Head ${i.head_ref||''} · ${i.head_commit||''}`),el('p',{
        class:'meta'
      },`Base ${i.base_ref||''} · ${i.base_commit||''}`),el('p',{
        class:'body'
      },'发布使用配置 Source 当前 Checkout 或已有权威 Worktree 中的干净提交。合并将更新 Git Base 分支；Base 必须未被任何 Worktree 检出。'),merged?null:el('label',{},'候选来源',candidateKind),merged?null:worktree.node,el('div',{
        class:'actions'
      },merged?el('span',{class:'badge'},'已合并，候选操作已结束'):button('发布候选',()=>execute({
        type:'publish',number:i.number,candidate:candidateKind.value==='source'?{type:'source'}:{type:'worktree',worktreeId:worktree.input.value}
      },i.revision)),merged?null:button('合并已发布候选',async()=>{
        if(await confirmAction(`将 PR #${i.number} 的已发布候选合并到 ${i.base_ref}？`))await execute({
          type:'merge',number:i.number
        },i.revision);
      },'danger'),button('读取真实 Git Diff',async()=>{const result=await api('/api/control/items/'+i.number+'/diff');diff.replaceChildren(el('pre',{},result.diff||'无差异'));})),diff,raw(v.merge,'持久合并记录'));
    }
    else{
      candidate=panel('关联 Pull Request',linked.node,button('关联',async()=>{
        const pr=await api('/api/control/items/'+Number(linked.input.value));await execute({
          type:'link',issue:i.number,pr:pr.item.number,expectedPrRevision:pr.item.revision
        },i.revision);
      }));
    }
    for(const f of [title,body,comment,profile,reply,linked]){f.input.dataset.draft='true';f.input.dataset.initialValue=f.input.value;}
    detail.replaceChildren(panel(null,heading,el('div',{
      class:'meta'
    },`Revision ${i.revision} · ${i.updated_at} · 作者 ${i.author}`),el('div',{
      class:'body'
    },i.body),raw(i,'完整对象')),discussion,controls,candidate);
  }
  const create=panel('创建协作对象',el('div',{
    class:'form-grid'
  },el('label',{},'类型',kind),createTitle.node,createBody.node),el('div',{
    class:'toolbar'
  },head.node,base.node,issue.node),button('创建',async()=>{
    const related=kind.value==='pr'&&issue.input.value?await api('/api/control/items/'+Number(issue.input.value)):null;
    await execute(kind.value==='issue'?{
      type:'createIssue',title:createTitle.input.value,body:createBody.input.value
    }
    :{
      type:'createPR',title:createTitle.input.value,body:createBody.input.value,head:head.input.value,base:base.input.value,...(issue.input.value?{
        issue:Number(issue.input.value),issueRevision:related.item.revision
      }
      :{})
    });
  },'primary'));
  for(const f of [createTitle,createBody,head,base,issue]){f.input.dataset.draft='true';f.input.dataset.initialValue=f.input.value;}
  function updateCreateType(){for(const f of [head,base,issue])f.node.hidden=kind.value!=='pr';}
  kind.addEventListener('change',updateCreateType);updateCreateType();
  const lookup=field('请求 ID');
  content.replaceChildren(el('p',{
    class:'meta'
  },'对象与运行状态来自当前协作空间；观测历史可在 Trace、日志和指标中查看。'),list,el('div',{
    class:'split'
  },create,panel('查询控制回执',lookup.node,button('查询',async()=>renderReceipt(await api('/api/control/receipts/'+encodeURIComponent(lookup.input.value||lastRequest||'')))))),receipt,detail);
  await reload();
}
route();
