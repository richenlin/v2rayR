const $ = s => document.querySelector(s);
const labels = ['连接','节点','订阅','测速','设置'];
const icons = ['M9 2v7M5 4a6 6 0 1 0 8 0','M3 4h12v4H3zM3 11h12v4H3z','M4 3h10v12H4zM7 7h4m-4 3h4','M3 14h12M4 11l4-4 3 2 4-5','M9 3l5 3v6l-5 3-5-3V6zM9 7v4'];
const initial = [{name:'东京 · 线路 02',protocol:'VLESS',delay:42},{name:'新加坡 · 线路 01',protocol:'Hysteria2',delay:68},{name:'香港 · 线路 03',protocol:'Trojan',delay:81}];
const modes = {'规则':'按分流规则决定直连或代理。','全局':'受接管流量默认走代理；保留局域网例外。','直连':'受接管流量不经代理节点。'};
let nodes=[...initial], page=0, active=0, state='on', mode='规则', ingress='系统代理', fault=false, message='', tested=false, theme=matchMedia('(prefers-color-scheme:dark)').matches?'dark':'light';
function themeUpdate(){document.documentElement.dataset.theme=theme;$('#theme').textContent=theme==='dark'?'切换浅色':'切换深色'}
function openImport(){$('#source').value='';$('#validation').textContent='';$('#preview').hidden=true;$('#import').textContent='预览示例';$('#dialog').showModal()}
function say(text){$('#announce').textContent=text}
const trace='<svg class="trace" viewBox="0 0 310 240" preserveAspectRatio="xMidYMid slice" aria-hidden="true"><g fill="none" stroke="#83b69d" stroke-width="1"><path d="M12 201 75 142H127L186 80h76l49-46M39 225l58-57h50l65-65h51l49-46M66 251l55-57h48l63-65h52l28-29M-15 176l65-60h53l66-62h74l61-51M-40 147l65-60h55l64-62h73l61-51"/><circle cx="186" cy="80" r="5" fill="#123e34"/><circle cx="147" cy="168" r="4" fill="#d9f6a9"/><circle cx="103" cy="116" r="4" fill="#123e34"/></g></svg>';
const rows=()=>nodes.map((n,i)=>'<div class="row" data-row><div class="name"><b>'+n.name+'</b><br><small>'+n.protocol+(i===active?' · 当前节点':'')+'</small></div><span class="mono">'+n.delay+' ms</span><button data-node="'+i+'" '+(i===active||state==='busy'?'disabled':'')+'>'+ (i===active?'已选用':state==='on'?'切换':'选用')+'</button></div>').join('');
const titles=['连接概览','节点','订阅','测速','设置'];
const subtitles=['管理当前连接、分流策略与常用节点。','查找线路并明确选用，选用前可查看延迟。','管理节点来源，查看上次更新结果。','查看线路延迟；该结果不代表应用流量已接管。','管理接入、分流与桌面偏好；设置只在原型会话内生效。'];
function render(focus){
 $('#crumb').textContent=labels[page];
 $('#nav').innerHTML=labels.map((n,i)=>'<button data-page="'+i+'" '+(page===i?'aria-current="page"':'')+'><svg viewBox="0 0 18 18" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" aria-hidden="true"><path d="'+icons[i]+'"/></svg>'+n+'</button>').join('');
 const online=state==='on', busy=state==='busy', has=nodes.length>0;
 const title=busy?'正在连接':state==='degraded'?'仅本地可用':online?'已连接':'未连接';
 const info=busy?'正在检查代理连通性…':state==='degraded'?'系统代理设置失败，应用流量未接管。':online?(ingress==='本地端口'?'本地端口已就绪，应用流量未接管。':mode==='直连'?'系统代理已接管，流量按直连策略处理。':'系统代理已接管，按'+mode+'模式分流。'):'连接后将使用当前接入方式与分流策略。';
 const displayTitle=online&&ingress==='本地端口'?'仅本地可用':title;
 const alert=message?'<div class="notice" role="status">'+message+'</div>':'';
 let body='';
 if(page===0)body=alert+'<div class="overview"><section class="hero" aria-label="当前连接">'+trace+'<div><div class="eyebrow">01 / CONNECTION</div><h2>'+displayTitle+'</h2><p>'+info+'</p></div><div><p>'+ (has?nodes[active].name+' · '+nodes[active].protocol:'尚未添加节点')+'</p><div class="actions"><button class="primary" id="connect" '+(busy?'disabled':'')+'>'+(!has?'导入节点':busy?'连接中…':online?'断开连接':state==='degraded'?'重试接管':'连接')+'</button>'+(has?'<button class="secondary" data-page="1">更换节点</button>':'')+(state==='degraded'?'<button class="secondary" id="disconnect">停止本地代理</button>':'')+'</div></div></section><section class="panel"><div class="section-head"><h2>流量概况</h2><small>最近 60 秒</small></div><div class="metrics"><div class="metric"><small>下载</small><strong>'+(online?'2.4 MB/s':'—')+'</strong></div><div class="metric"><small>上传</small><strong>'+(online?'186 KB/s':'—')+'</strong></div></div><svg class="chart" viewBox="0 0 300 68" preserveAspectRatio="none" aria-hidden="true"><path d="M0 16h300M0 40h300M0 64h300" stroke="var(--line)" fill="none"/>'+(online?'<path d="M0 57l18-3 18 3 18-18 18 7 18-5 18-19 18 7 18-13 18 5 18 18 18-7 18 4 18-13 18 2 18-13 30 6" stroke="var(--focus)" stroke-width="2" fill="none"/>':'')+'</svg><small>'+(online?'演示采样 · 仅供预览':'暂无采样')+'</small></section></div><div class="panel route"><div><label for="ingress">接入方式</label><select id="ingress" '+(busy?'disabled':'')+'>'+['系统代理','本地端口'].map(v=>'<option '+(v===ingress?'selected':'')+'>'+v+'</option>').join('')+'</select><p class="muted">决定系统流量如何进入本应用。</p></div><div><label for="mode">分流策略</label><select id="mode" '+(busy?'disabled':'')+'>'+Object.keys(modes).map(v=>'<option '+(v===mode?'selected':'')+'>'+v+'</option>').join('')+'</select><p class="muted">'+modes[mode]+'</p></div></div><section class="panel"><div class="section-head"><h2>常用节点</h2><button data-page="1">全部节点 →</button></div>'+(has?rows():'<p class="muted">导入后即可选用节点。</p>')+'</section>';
 if(page===1)body='<div class="tools"><input id="search" aria-label="搜索节点" placeholder="搜索节点或协议"><button class="primary" data-import>导入</button></div><section class="panel"><div class="section-head"><h2>所有节点</h2><small>'+nodes.length+' 条</small></div>'+rows()+'<p class="muted" id="no-results" '+(has?'hidden':'')+'>没有匹配的节点。</p></section>';
 if(page===2)body=alert+'<section class="panel"><div class="section-head"><h2>'+(has?esc(saved.sourceName):'暂无订阅')+'</h2><button data-import class="primary">添加订阅</button></div><p class="muted">'+(has?'配额未提供 · '+nodes.length+' 个节点':'添加订阅以管理节点来源。')+'</p>'+(has?'<div class="actions"><button id="refresh">模拟更新</button><button data-source-pref>编辑订阅</button></div>':'')+'</section>';
 if(page===3)body='<section class="panel"><div class="section-head"><h2>延迟结果</h2><button id="test" class="primary">模拟测速</button></div>'+(tested&&has?nodes.map(n=>'<div class="row"><b>'+n.name+'</b><span class="mono">'+n.delay+' ms</span></div>').join(''):'<p class="muted">'+(has?'尚未运行本次测速。':'请先导入节点。')+'</p>')+'</section>';
 if(page===4)body=settingsBody();
 $('#page').innerHTML='<div class="title"><div class="eyebrow">NETWORK CONTROL / 0'+(page+1)+'</div><h1 tabindex="-1">'+titles[page]+'</h1><p class="muted">'+subtitles[page]+'</p></div>'+body;
 if(focus)($(focus)||$('h1')).focus();
}
document.addEventListener('click',e=>{
 const b=e.target.closest('button');if(!b)return;
 if(b.dataset.page!==undefined){page=+b.dataset.page;message='';if(page===4){settingQuery='';settingMessage='';if(group==='source')group='general';}render('h1');return}
 if(b.hasAttribute('data-import')){openImport();return}
 if(b.id==='theme'){theme=theme==='dark'?'light':'dark';themeUpdate();return}
 if(b.hasAttribute('data-source-pref')){page=4;group='source';settingQuery='';settingMessage='';render('#group-heading');return}
 if(b.dataset.group){group=b.dataset.group;settingQuery='';settingMessage='';const field=fields.find(f=>f[0]===b.dataset.find);render(field&&visible(field)?'#pref-'+field[0]:'#group-heading');if(field&&!visible(field)){$('#pref-status').textContent='此项取决于 '+fields.find(f=>f[0]===field[6].split('=')[0])[1]+'，请先启用相关选项。';}return}
 if(b.id==='pref-reset'){groupForKey($('#pref-form [data-pref]').dataset.pref)[5].forEach(([k])=>draft[k]=saved[k]);settingMessage='已撤销本组未保存修改。';render('#pref-reset');return}
 if(b.dataset.demo){const x=b.dataset.demo;nodes=x==='empty'?[]:[...initial];active=0;ingress='系统代理';mode='规则';fault=x==='failure';state=x==='on'?'on':fault?'degraded':'off';message='';page=0;render('h1');return}
 if(b.id==='disconnect'||(b.id==='connect'&&state==='on')){state='off';fault=false;message='';render('#connect');say('已断开');return}
 if(b.id==='connect'){
  if(!nodes.length){openImport();return}
  if(state==='degraded')fault=false;
  state='busy';message='';render('h1');say('正在连接');
  setTimeout(()=>{if(state!=='busy')return;state=fault?'degraded':'on';render(page===0?'#connect':null);say(fault?'系统代理未接管':'模拟连接已就绪')},650);
 }
 if(b.dataset.node!==undefined){active=+b.dataset.node;render('h1');say('已选用 '+nodes[active].name)}
 if(b.id==='refresh'){message='模拟更新完成，保留现有节点。';render('#refresh')}
 if(b.id==='test'){tested=true;render('#test');say('模拟测速完成')}
});
document.addEventListener('input',e=>{
 if(e.target.id==='setting-search'){if(e.isComposing)return;settingQuery=e.target.value;const pos=e.target.selectionStart;render('#setting-search');$('#setting-search').setSelectionRange(pos,pos);return}
 if(e.target.dataset.pref&&!['checkbox','select-one'].includes(e.target.type)){draft[e.target.dataset.pref]=e.target.value;$('#pref-status').textContent=isDirty(groupForKey(e.target.dataset.pref))?'有未保存的修改。':'当前分组无未保存修改。';return}
 if(e.target.id==='source'){$('#preview').hidden=true;$('#import').textContent='预览示例';$('#validation').textContent='';return}
 if(e.target.id!=='search')return;
 let count=0;const query=e.target.value.trim().toLowerCase();
 document.querySelectorAll('[data-row]').forEach(row=>{row.hidden=!row.textContent.toLowerCase().includes(query);if(!row.hidden)count++});
 $('#no-results').hidden=count>0;
});
document.addEventListener('compositionend',e=>{if(e.target.id==='setting-search')e.target.dispatchEvent(new Event('input',{bubbles:true}))});
document.addEventListener('change',e=>{
 if(e.target.dataset.pref&&['checkbox','select-one'].includes(e.target.type)){const k=e.target.dataset.pref;draft[k]=e.target.type==='checkbox'?e.target.checked:e.target.value;settingMessage='';render('#pref-'+k);return}
 if(e.target.id==='mode'){mode=e.target.value;render('#mode');say('分流策略已应用：'+mode)}
 if(e.target.id==='ingress'){ingress=e.target.value;if(state==='degraded'){state=ingress==='本地端口'?'on':'degraded';fault=state==='degraded'}render('#ingress');say('接入方式：'+ingress)}
});
$('#import').addEventListener('click',()=>{if(!$('#source').value.trim()){$('#validation').textContent='请输入任意示例地址。';return}if($('#preview').hidden){$('#validation').textContent='';$('#preview').hidden=false;$('#import').textContent='确认导入';return}nodes=[...initial];active=0;$('#source').value='';$('#dialog').close();page=0;message='已载入 3 个示例节点。';render('#connect');say('示例节点已导入')});
document.addEventListener('submit',e=>{if(e.target.id==='pref-form'){e.preventDefault();applySettings()}});
themeUpdate();render();
