// [key, label, control, default, detail, options or bounds, dependency]
const settingsGroups = [
 ['general','启动与托盘','决定应用何时启动，以及关闭窗口后是否继续运行。','保存偏好','M1',[
 ['autostart','登录时启动','check',false,'适合每天使用；默认关闭。'],
 ['autoconnect','启动后自动连接','check',false,'仅在已有可用节点时连接；可能改变系统代理。'],
 ['close','关闭窗口时','select','留在托盘','没有可用托盘时退出；退出会恢复网络设置。',['留在托盘','退出应用']]
 ]],
 ['proxy','代理接入','配置本机工具或同一局域网的设备如何连接。','保存并模拟重连','M1',[
 ['port','本地混合端口','number',0,'0 为自动（优先 10808）；只有本机工具依赖固定端口时才修改。',[0,65535]],
 ['bypass','绕过本地地址','check',true,'默认让本地服务不经过系统代理。'],
 ['exceptions','额外绕过地址','area','','公司内网或本地服务有特殊要求时填写，每行一个域名或 IP。'],
 ['lan','允许局域网设备接入','check',false,'启用后需设置访问凭据；请只在可信网络使用。'],
 ['lanUser','局域网用户名','text','','仅输入示例值。',null,'lan'],
 ['lanPass','局域网密码','password','','仅输入示例值；原型只在当前页面会话保存。',null,'lan']
 ]],
 ['routing','分流与 DNS','默认规则适合日常使用；有明确的网络要求时再改。','保存并模拟重连','M1 / M3',[
 ['preset','规则模式使用的预设','select','绕过大陆','全局 / 直连模式在连接页切换。',['绕过大陆','默认代理']],
 ['dnsMode','DNS 方案','select','自动','自动模式按当前内核和接入方式选择。',['自动','自定义']],
 ['directDns','直连 DNS','text','https://dns.alidns.com/dns-query','填写可用的 HTTPS DNS 地址。',null,'dnsMode=自定义'],
 ['remoteDns','代理 DNS','text','https://dns.cloudflare.com/dns-query','填写可通过当前代理访问的 HTTPS DNS 地址。',null,'dnsMode=自定义']
 ]],
 ['subs','订阅默认值','仅控制没有单独设置更新计划的订阅。','保存偏好','M1',[
 ['hours','自动更新间隔（小时）','number',12,'应用运行时更新；0 为仅手动更新。',[0,168]]
 ]],
 ['source','当前订阅','只编辑当前示例订阅，不覆盖全局默认值。','保存订阅偏好','M1 / M3',[
 ['sourceName','订阅名称','text','示例订阅','便于区分多个来源。'],
 ['sourceInterval','更新间隔（小时）','optionalNumber','','留空继承全局；0 表示仅手动。',[0,168]],
 ['sourceAdvanced','高级订阅兼容','check',false,'只在服务商要求特定客户端标识，或只想保留部分线路时打开。'],
 ['sourceUA','自定义 User-Agent','text','','留空使用 v2rayR 默认标识。',null,'sourceAdvanced'],
 ['include','仅保留匹配节点','text','','名称包含该关键词才保留；原型不执行筛选。',null,'sourceAdvanced'],
 ['excludePattern','排除匹配节点','text','','名称包含该关键词则排除；原型不执行筛选。',null,'sourceAdvanced']
 ]],
 ['look','外观','适配系统外观与个人阅读习惯。','保存偏好','M1 / M2',[
 ['theme','配色主题','select','跟随系统','可立即预览。',['跟随系统','浅色','深色']],
 ['language','界面语言','select','跟随系统','原型界面仅提供简体中文。',['跟随系统','简体中文','繁體中文','English']],
 ['font','界面字号','number',14,'结合系统缩放使用。',[12,20]]
 ]],
 ['demo','原型场景','只用于评审，不属于正式设置。','', '预览',[]]
];
const escapes = {'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'};
const esc = x => String(x).replace(/[&<>"']/g,ch=>escapes[ch]);
const fields=settingsGroups.flatMap(g=>g[5]);
const saved=Object.fromEntries(fields.map(([k,,,value])=>[k,value]));
const draft={...saved};
let group='general', settingQuery='', settingMessage='';
const visible=f=>{if(!f[6])return true;const [key,match]=f[6].split('=');return match?draft[key]===match:Boolean(draft[key])};
function fieldHtml(f){
 const [key,label,type,,hint,options]=f, val=draft[key], id='pref-'+key;
 const attrs=' id="'+id+'" data-pref="'+key+'" aria-describedby="'+id+'-hint"';
 const control=type==='check'?'<input type="checkbox"'+attrs+(val?' checked':'')+'>':type==='select'?'<select'+attrs+'>'+options.map(v=>'<option'+(v===val?' selected':'')+'>'+esc(v)+'</option>').join('')+'</select>':type==='area'?'<textarea rows="2"'+attrs+'>'+esc(val)+'</textarea>':type==='later'?'<span class="tag">'+esc(val)+' · 规划</span>':'<input type="'+(type==='optionalNumber'?'number':type)+'"'+attrs+(type==='password'?' autocomplete="off"':'')+' value="'+esc(val)+'"'+(['number','optionalNumber'].includes(type)?' min="'+options[0]+'" max="'+options[1]+'" step="1"':'')+'>';
 return '<div class="pref-row"><div>'+ (type==='later'?'<b>'+label+'</b>':'<label for="'+id+'">'+label+'</label>')+'<p class="muted" id="'+id+'-hint">'+hint+'</p></div>'+control+'</div>';
}
const groupForKey=k=>settingsGroups.find(g=>g[5].some(f=>f[0]===k));
const isDirty=g=>g[5].some(f=>draft[f[0]]!==saved[f[0]]);
function settingsBody(){
 const g=settingsGroups.find(x=>x[0]===group);
 const q=settingQuery.trim().toLowerCase();
 const results=q?settingsGroups.filter(x=>x[0]!=='source'||group==='source').map(x=>[x,x[5].filter(f=>[x[1],...f.slice(0,3),f[4]].some(v=>String(v).toLowerCase().includes(q)))]).filter(x=>x[1].length):[];
 return '<div class="settings-tools"><input id="setting-search" aria-label="搜索设置" placeholder="搜索设置，例如 DNS、端口、托盘" value="'+esc(settingQuery)+'"></div><div class="settings-layout"><nav class="settings-nav" aria-label="设置分类">'+settingsGroups.filter(x=>x[0]!=='source'||group==='source').map(x=>'<button data-group="'+x[0]+'" '+(!q&&group===x[0]?'aria-current="page"':'')+'>'+x[1]+(isDirty(x)?' <span class="draft-dot" aria-label="有未保存修改">•</span>':'')+'</button>').join('')+'</nav><div class="settings-content">'+(q?'<section class="panel"><h2>搜索结果</h2>'+(results.length?results.map(([x,list])=>list.map(f=>'<button class="pref-result" data-group="'+x[0]+'" data-find="'+f[0]+'"><b>'+f[1]+'</b><small>'+x[1]+'</small></button>').join('')).join(''):'<p class="muted">未找到匹配项，试试「代理」「DNS」或「更新」。</p>')+'</section>':
 '<section class="panel"><div class="section-head"><h2 tabindex="-1" id="group-heading">'+g[1]+'</h2><span class="tag">'+g[4]+'</span></div><p class="muted">'+g[2]+'</p>'+
 (group==='demo'?'<div class="actions"><button data-demo="empty">首启空状态</button><button data-demo="off">未连接</button><button data-demo="failure">系统代理失败</button><button data-demo="on">已连接</button></div>':
 '<form id="pref-form" novalidate>'+g[5].filter(visible).map(fieldHtml).join('')+'<div class="pref-footer"><p class="muted" role="status" id="pref-status">'+esc(settingMessage|| (isDirty(g)?'有未保存的修改。':'当前分组无未保存修改。'))+'</p><div class="actions"><button type="button" id="pref-reset">撤销修改</button><button class="primary" type="submit">'+g[3]+'</button></div></div></form>')+'</section>')+'</div></div>';
}
function applySettings(){
 if(state==='busy'){$('#pref-status').textContent='请等待当前模拟连接操作完成。';return}
 const g=settingsGroups.find(x=>x[0]===group);
 const invalid=(k,msg)=>{$('#pref-status').textContent=msg;$('#pref-'+k)?.focus();};
 for(const f of g[5].filter(visible)){
  const [k,label,type,, ,range]=f,v=draft[k];
  if((type==='number'||(type==='optionalNumber'&&v!==''))&&(v===''||!Number.isInteger(Number(v))||Number(v)<range[0]||Number(v)>range[1]))return invalid(k,label+'须为 '+range.join('–')+' 范围内的整数。');
  }
 if(group==='proxy'&&draft.lan){
   if(!draft.lanUser.trim())return invalid('lanUser','请输入局域网用户名。');
  if(!draft.lanPass.trim())return invalid('lanPass','请输入局域网密码。');
 }
 if(group==='routing'&&draft.dnsMode==='自定义')for(const k of ['directDns','remoteDns']){try{const url=new URL(draft[k]);if(url.protocol!=='https:'||url.username||url.password)throw Error()}catch{return invalid(k,'请输入不带账号密码的 HTTPS DNS 地址。')}}
 if(group==='source'&&!draft.sourceName.trim())return invalid('sourceName','请输入订阅名称。');
 const changed=isDirty(g);
 g[5].forEach(([k])=>saved[k]=draft[k]);
 if(group==='look'){
  theme=saved.theme==='跟随系统'?(matchMedia('(prefers-color-scheme:dark)').matches?'dark':'light'):saved.theme==='深色'?'dark':'light';
  themeUpdate(); document.body.style.fontSize=saved.font+'px';
 }
 const reconnect=changed&&g[3].includes('重连')&&state==='on';
 if(reconnect)state='busy';
 settingMessage=changed?(reconnect?'正在模拟重连…':'已保存到当前原型会话。'):'没有需要保存的修改。';
 render('#pref-form button[type=submit]');
 if(reconnect)setTimeout(()=>{
  if(state!=='busy')return;
  state='on';
  if(page===4&&group===g[0]&&!settingQuery){settingMessage='模拟重连完成。';$('#pref-status').textContent=settingMessage}
  else if(page===0)render('#connect');
  say('模拟重连完成');
 },450);
}
