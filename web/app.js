'use strict';
const $=id=>document.getElementById(id);
let config, selected=0, learning=false, sequence=0, dirty=false, running=false;
const defaults=[69,98,55,74,71,72,73,78,75,76,77,83,79,80,81,96,82,14,28];
const physicalKeys=['1','2','3','4','5','6','7','8','9','0','•','Enter','−','+','Без подписи','↑','←','→','↓','Колёсико ↶','Колёсико ↷','Нажатие колёсика'];
const actionNames={chord:'Сочетание',macro:'Макрос',disabled:'Отключена'};
const codes=()=>[...(profile().layout||defaults),...(profile().wheel||[null,null,null])];
const profile=()=>config.profiles.find(p=>p.name===config.active);
function message(text,error=false){$('message').textContent=text;$('message').classList.toggle('error',error)}
async function api(path,body){const r=await fetch('/api/'+path,body===undefined?{}:{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});const d=await r.json();if(!r.ok)throw Error(d.error||'Ошибка запроса');return d}
function changed(){dirty=true;$('dirty').textContent='Есть изменения'}
function render(){const select=$('profile');select.replaceChildren();for(const p of config.profiles){const o=new Option(p.name,p.name);select.add(o)}select.value=config.active;renderKeys();edit()}
function renderKeys(){
 const root=$('keyboard');root.replaceChildren();
 const side=document.createElement('span');side.className='side-control';side.title='Колёсико: вращение в обе стороны и нажатие';side.setAttribute('aria-hidden','true');root.append(side);
 const pad=document.createElement('span');pad.className='direction-pad';pad.setAttribute('aria-hidden','true');root.append(pad);
 codes().slice(0,19).forEach((code,i)=>{
  const b=document.createElement('button');b.type='button';b.dataset.slot=i;
  b.className=`key slot-${i}`+(i===selected?' selected':'');
  const binding=profile().bindings[code];
  b.setAttribute('aria-label',`${physicalKeys[i]}: ${binding?.label||actionNames[binding?.kind]||'Обычная клавиша'}, код ${code}`);
  b.setAttribute('aria-pressed',String(i===selected));
  b.title=`${physicalKeys[i]} · код ${code} · ${binding?.label||actionNames[binding?.kind]||'Обычная клавиша'}`;
  const face=document.createElement('span');face.className='key-face';face.textContent=i===14?'':physicalKeys[i];b.append(face);
  const s=document.createElement('small');s.textContent=binding?.label||actionNames[binding?.kind]|| (i===14?'Без подписи':String(code));b.append(s);
  b.onclick=()=>{selected=i;if(learning)api('learn',{enabled:false}).catch(e=>message(e.message,true));learning=false;$('learn').textContent='◎ Определить по нажатию';renderKeys();edit()};root.append(b);
 });
 const wheel=$('wheelControls');wheel.replaceChildren();
 for(let i=19;i<22;i++){
  const b=document.createElement('button');b.type='button';b.dataset.slot=i;
  b.className='wheel-action'+(selected===i?' selected':'');
  const code=codes()[i];b.textContent=physicalKeys[i]+(code===null?' · код не определён':` · ${code}`);
  b.setAttribute('aria-pressed',String(selected===i));
  b.onclick=()=>{if(learning)api('learn',{enabled:false}).catch(e=>message(e.message,true));learning=false;selected=i;$('learn').textContent='◎ Определить по нажатию';renderKeys();edit()};wheel.append(b);
 }
 const mark=document.createElement('span');mark.className='deck-mark';mark.textContent='GAMING / 19 KEY';mark.setAttribute('aria-hidden','true');root.append(mark);
}
function edit(){const code=codes()[selected],b=profile().bindings[code]||{};$('keyTitle').textContent=selected<14?`Клавиша ${physicalKeys[selected]}`:selected===14?'Широкая клавиша':selected<19?`Направление ${physicalKeys[selected]}`:physicalKeys[selected];$('source').value=code??'';$('label').value=b.label||'';$('kind').value=b.kind||'passthrough';$('keys').value=(b.keys||[]).join(', ');$('steps').value=(b.steps||[]).map(s=>`${s.keys.join(',')} @ ${s.delay}`).join('\n');kind()}
function kind(){$('chordEditor').hidden=$('kind').value!=='chord';$('macroEditor').hidden=$('kind').value!=='macro'}
function parseKeys(s){const parts=s.split(',').map(s=>s.trim());if(parts.some(s=>!/^\d+$/.test(s)))throw Error('Введите коды через запятую');const a=parts.map(Number);if(!a.length||a.length>8||a.some(n=>n<1||n>767)||new Set(a).size!==a.length)throw Error('Нужно 1–8 разных кодов от 1 до 767');return a}
$('kind').onchange=kind;
$('apply').onclick=()=>{try{const source=Number($('source').value);if(!Number.isInteger(source)||source<1||source>767)throw Error('Неверный исходный код');if(codes().some((c,i)=>i!==selected&&c===source))throw Error('Этот код уже назначен другой кнопке');const old=codes()[selected],k=$('kind').value;const b={kind:k,label:$('label').value};if(k==='chord')b.keys=parseKeys($('keys').value);if(k==='macro'){b.steps=$('steps').value.trim().split('\n').map(line=>{const a=line.split('@');if(a.length!==2||!/^\d+$/.test(a[1].trim()))throw Error('Формат шага: 29,46 @ 100');const delay=Number(a[1]);if(delay>2000)throw Error('Пауза до 2000 мс');return{keys:parseKeys(a[0]),delay}});if(b.steps.length>64||b.steps.reduce((a,s)=>a+s.delay,0)>5000)throw Error('Максимум 64 шага и 5000 мс пауз')}if(!profile().layout)profile().layout=[...defaults];if(selected<19)profile().layout[selected]=source;else{if(!profile().wheel)profile().wheel=[null,null,null];profile().wheel[selected-19]=source;}delete profile().bindings[old];if(k!=='passthrough')profile().bindings[source]=b;changed();renderKeys();message('Назначение применено к профилю. Нажмите «Сохранить», чтобы включить его.')}catch(e){message(e.message,true)}};
$('save').onclick=async()=>{try{await api('config',config);dirty=false;$('dirty').textContent='Сохранено';message('Профиль сохранён и применён')}catch(e){message(e.message,true)}};
$('profile').onchange=()=>{config.active=$('profile').value;changed();render()};
$('addProfile').onclick=()=>{const name=prompt('Название нового профиля');if(!name)return;if(config.profiles.some(p=>p.name===name)){message('Такое имя уже существует',true);return}config.profiles.push({name,bindings:{},layout:[...defaults]});config.active=name;changed();render()};
$('deleteProfile').onclick=()=>{if(config.profiles.length===1){message('Нужен хотя бы один профиль',true);return}if(!confirm('Удалить текущий профиль?'))return;config.profiles=config.profiles.filter(p=>p.name!==config.active);config.active=config.profiles[0].name;changed();render()};
$('export').onclick=()=>{const u=URL.createObjectURL(new Blob([JSON.stringify(config,null,2)],{type:'application/json'}));const a=document.createElement('a');a.href=u;a.download='keymasterbind-profiles.json';a.click();setTimeout(()=>URL.revokeObjectURL(u),1000)};
$('import').onchange=async e=>{try{const file=e.target.files[0];if(!file)return;if(file.size>1000000)throw Error('Файл слишком большой');const c=JSON.parse(await file.text());if(!confirm('Заменить сохранённые профили содержимым файла?'))return;await api('config',c);config=c;dirty=false;selected=0;render();message('Профили импортированы')}catch(e){message(e.message,true)}finally{e.target.value=''}};
$('auto').onclick=async()=>{try{await api('auto',{});message('Автоподключение включено — клавиатура будет выбрана автоматически')}catch(e){message(e.message,true)}};
$('start').onclick=async()=>{try{if(dirty)throw Error('Сначала сохраните изменения профиля');if(!$('device').value)throw Error('Устройство не найдено');await api('start',{device:$('device').value});message('Запускаю перехват…')}catch(e){message(e.message,true)}};
$('stop').onclick=async()=>{try{await api('stop',{});learning=false;message('Перехват останавливается')}catch(e){message(e.message,true)}};
$('learn').onclick=async()=>{try{if(!running)throw Error('Сначала включите перехват устройства');const enabled=!learning;await api('learn',{enabled});learning=enabled;$('learn').textContent=learning?'Нажмите физическую клавишу…':'◎ Определить по нажатию'}catch(e){message(e.message,true)}};
for(const b of document.querySelectorAll('[data-keys]'))b.onclick=()=>{$('keys').value=b.dataset.keys};
for(const b of document.querySelectorAll('[data-tab]'))b.onclick=()=>{for(const n of document.querySelectorAll('[data-tab]'))n.classList.toggle('active',n===b);$('editor').hidden=b.dataset.tab!=='editor';$('firmware').hidden=b.dataset.tab!=='firmware'};
let lastError='';async function poll(){try{const s=await api('status');running=s.running;$('badge').textContent=running?'● Перехват включён':s.devices.some(d=>d.readable)?'○ Клавиатура найдена':s.devices.length?'○ Нет доступа к клавиатуре':'○ Устройство недоступно';$('badge').classList.toggle('on',running);$('connectionText').textContent=running?`Подключено: ${s.device}${s.auto_connect?' · переподключение автоматически':''}`:(s.auto_connect?'Автоподключение: ':'Остановлено. ')+(s.detection_message||'Ожидание клавиатуры');const signature=JSON.stringify(s.devices);if($('device').dataset.signature!==signature){const old=$('device').value;$('device').replaceChildren(...s.devices.map(d=>new Option(`${d.name} · ${d.path}${d.readable?'':' · нет доступа'}`,d.path)));if(s.devices.some(d=>d.path===old))$('device').value=old;else if(s.preferred_device)$('device').value=s.preferred_device;$('device').dataset.signature=signature}if(s.error&&s.error!==lastError)message(s.error,true);lastError=s.error;if(s.sequence!==sequence){sequence=s.sequence;$('lastKey').textContent=s.last_key;if(learning){$('source').value=s.last_key;learning=false;$('learn').textContent='◎ Определить по нажатию';message('Код определён. Выберите действие и примените к кнопке.')}const i=codes().indexOf(s.last_key);if(i>=0){const key=document.querySelector(`[data-slot="${i}"]`);key.classList.add('flash');setTimeout(()=>key.classList.remove('flash'),250)}}}catch(e){message('Нет связи с сервером: '+e.message,true)}finally{setTimeout(poll,500)}}
window.onbeforeunload=e=>{if(dirty){e.preventDefault();e.returnValue=''}};
(async()=>{try{config=await api('config');render();poll()}catch(e){message(e.message,true)}})();
