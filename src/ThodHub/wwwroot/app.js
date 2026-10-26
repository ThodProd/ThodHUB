/* ThodHUB shell. Module code only receives the permission-checked module bridge. */
'use strict';
const $ = s => document.querySelector(s);
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const paths = {
  library:'<rect x="3" y="3" width="7" height="7" rx="2"/><rect x="14" y="3" width="7" height="7" rx="2"/><rect x="3" y="14" width="7" height="7" rx="2"/><rect x="14" y="14" width="7" height="7" rx="2"/>',
  refresh:'<path d="M20 7a9 9 0 0 0-15-2L3 7m0-5v5h5M4 17a9 9 0 0 0 15 2l2-2m0 5v-5h-5"/>',
  plus:'<path d="M12 5v14M5 12h14"/>', close:'<path d="m6 6 12 12M18 6 6 18"/>',
  settings:'<path d="m9 3-1 3-3 1-2 3 2 2-1 3 3 2 3-1 2 3 3-1 1-3 3-1 1-3-2-2 1-3-3-2-3 1-2-2z"/><circle cx="11" cy="11" r="3"/>',
  search:'<circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 5 5"/>',
  resume:'<path d="M7 3h8l4 4v14H5V3zM14 3v5h5M8 13h8M8 17h6"/>',
  mixer:'<path d="M5 3v5m0 4v9m7-18v11m0 4v3m7-18v3m0 4v11"/><rect x="2" y="8" width="6" height="4" rx="1"/><rect x="9" y="14" width="6" height="4" rx="1"/><rect x="16" y="6" width="6" height="4" rx="1"/>',
  vpn:'<path d="m12 2 8 3v6c0 5-8 11-8 11S4 16 4 11V5z"/><path d="m8 11 3 3 5-6"/>',
  monitor:'<rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8m-4-4v4M5 10h3l2-4 4 8 2-4h3"/>',
  copy:'<rect x="8" y="8" width="12" height="13" rx="2"/><path d="M15 8V3H3v13h5"/>',
  code:'<path d="m8 6-6 6 6 6m8-12 6 6-6 6M14 3l-4 18"/>',
  github:'<path d="M9 19c-5 1-5-3-7-3m14 6v-4c0-1-.3-2-1-2.5 4-.5 6-2 6-6a5 5 0 0 0-1.4-3.5 5 5 0 0 0-.1-3.5S18 2 15 4a14 14 0 0 0-6 0C6 2 4.5 2.5 4.5 2.5A5 5 0 0 0 4.4 6 5 5 0 0 0 3 9.5c0 4 2 5.5 6 6-.7.5-1 1.5-1 2.5v4"/>',
  folder:'<path d="M3 5h7l2 3h9v12H3z"/>', check:'<path d="m5 12 4 4L19 6"/>',
  arrow:'<path d="M5 12h14m-5-5 5 5-5 5"/>', download:'<path d="M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5"/>',
  up:'<path d="m6 15 6-6 6 6"/>', down:'<path d="m6 9 6 6 6-6"/>', info:'<circle cx="12" cy="12" r="9"/><path d="M12 11v6m0-10v1"/>'
};
paths.expand='<path d="M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5"/>';
paths.external='<path d="M14 3h7v7m0-7L10 14M10 3H3v18h18v-7"/>';
paths.trash='<path d="M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7m4-7v7"/>';
const icon = (name, cls='') => `<svg class="${cls}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${paths[name] || paths.library}</svg>`;
const appIcon = app => `<span class="app-icon ${esc(app.icon)}">${app.iconUrl?`<img src="${esc(app.iconUrl)}" data-fallback-icon="${esc(app.icon)}" alt="">`:icon(app.icon)}</span>`;
function artwork(app) {
  if(app.artworkUrl) return `<img src="${esc(app.artworkUrl)}" data-fallback-icon="${esc(app.icon)}" alt="" loading="lazy">`;
  const open='<svg class="art" viewBox="0 0 280 150" fill="none" aria-hidden="true">', end='</svg>';
  switch(app.icon) {
    case 'resume': return open+'<path fill="#e8eef4" d="M0 0h280v150H0z"/><g transform="rotate(-7 135 77)"><rect x="83" y="19" width="107" height="137" rx="4" fill="#d2dce5"/></g><g transform="rotate(5 145 75)"><rect x="91" y="15" width="107" height="137" rx="4" fill="#fff"/><circle cx="115" cy="40" r="9" fill="#dce7ee"/><rect x="132" y="34" width="46" height="4" rx="2" fill="#54758d"/><rect x="132" y="43" width="30" height="3" rx="1.5" fill="#b9c7d2"/><path d="M107 66h74M107 88h74M107 110h74" stroke="#e8edf1"/><path d="M107 59h32M107 81h32M107 103h32" stroke="#91a8ba" stroke-width="3"/><path d="M107 72h60M107 94h60M107 116h60" stroke="#d2dce5" stroke-width="2"/></g>'+end;
    case 'mixer': return open+'<path fill="#eeeaf4" d="M0 0h280v150H0z"/><rect x="57" y="24" width="166" height="120" rx="12" fill="#fdfcff" stroke="#ded6e8"/><path d="M88 48v70m35-70v70m35-70v70m35-70v70" stroke="#e4deed" stroke-width="5" stroke-linecap="round"/><path d="M88 90v28m35-51v51m35-20v20m35-64v64" stroke="#baa5d1" stroke-width="5" stroke-linecap="round"/><rect x="79" y="79" width="18" height="13" rx="4" fill="#fff" stroke="#b1a0c6"/><rect x="114" y="57" width="18" height="13" rx="4" fill="#fff" stroke="#b1a0c6"/><rect x="149" y="90" width="18" height="13" rx="4" fill="#fff" stroke="#b1a0c6"/><rect x="184" y="45" width="18" height="13" rx="4" fill="#fff" stroke="#b1a0c6"/>'+end;
    case 'vpn': return open+'<path fill="#e9f1ec" d="M0 0h280v150H0z"/><circle cx="140" cy="78" r="60" stroke="#d1e2d7"/><ellipse cx="140" cy="78" rx="32" ry="60" stroke="#d1e2d7"/><path d="M80 78h120m-111-30h103m-103 60h103" stroke="#d1e2d7"/><path d="m140 29 36 13v31c0 27-36 48-36 48s-36-21-36-48V42z" fill="#fff" stroke="#a3bfae" stroke-width="1.4"/><path d="m125 74 10 10 22-23" stroke="#6b9b7d" stroke-width="5" stroke-linecap="round" stroke-linejoin="round"/>'+end;
    case 'monitor': return open+'<path fill="#e8eff4" d="M0 0h280v150H0z"/><rect x="48" y="28" width="184" height="105" rx="9" fill="#f9fcff" stroke="#c4d5e0"/><path d="M64 57h152M64 77h152M64 97h152M84 43v75m32-75v75m32-75v75m32-75v75m32-75v75" stroke="#e4edf4"/><path d="m64 93 23-3 14-26 18 38 19-54 17 38 15-7 17 16 29-17" stroke="#739cba" stroke-width="3" stroke-linejoin="round"/><circle cx="216" cy="78" r="4" fill="#739cba"/>'+end;
    case 'copy': return open+'<path fill="#f4efe4" d="M0 0h280v150H0z"/><path d="M80 52h47l13 13h61v63H80z" fill="#e7d8b7" stroke="#d6c69f"/><rect x="104" y="23" width="77" height="92" rx="5" transform="rotate(7 104 23)" fill="#fff" stroke="#e2d7be"/><path d="M119 45h43m-43 12h34m-34 12h43m-43 12h28" stroke="#c8b790" stroke-width="3" stroke-linecap="round"/><path d="M69 72h135l-10 56H80z" fill="#f1e3c4" stroke="#d6c69f"/>'+end;
    default: return icon('library');
  }
}
let snapshot = { catalog:{apps:[]}, settings:{theme:'system',checkOnStartup:true,installed:[],sidebarOrder:[]}, native:false, hubVersion:'0.1.0', dataRoot:'' };
let route='library', category='Все', filter='', dragId=null, activeId=null, pendingInstall=null, syncing=false;
let toastTimer;
let removeId=null, fullscreen=false, moduleGeneration=0;
const downloads=new Map(), pending=new Map();
const isNative=!!window.chrome?.webview;
const native = (method, params={}) => {
  if(!isNative) return Promise.reject(new Error('Это предпросмотр. Системные действия доступны в приложении ThodHUB.'));
  return new Promise((resolve,reject)=>{
    const id=crypto.randomUUID();
    const timeout=setTimeout(()=>{pending.delete(id);reject(new Error('Время ожидания истекло.'));},(method==='app.install'||method.startsWith('backups.'))?16*60*1000:90*1000);
    pending.set(id,{resolve,reject,timeout});
    window.chrome.webview.postMessage({id,method,params});
  });
};
if(isNative) window.chrome.webview.addEventListener('message', e=>{
  const m=e.data;
  if(m.event==='fullscreen'){fullscreen=m.enabled;document.body.classList.toggle('module-fullscreen',fullscreen);return;}
  if(m.event==='notice'){notice(m.text);return;}
  if(m.event==='install.progress'){downloads.set(m.id,{progress:m.progress,stage:m.stage});updateProgress(m.id);return;}
  const p=pending.get(m.id);if(!p)return;
  pending.delete(m.id);clearTimeout(p.timeout);m.error?p.reject(new Error(m.error)):p.resolve(m.result);
});
function notify(text){$('#toast').textContent=text;$('#toast').hidden=false;clearTimeout(toastTimer);toastTimer=setTimeout(()=>$('#toast').hidden=true,5000);}
function notice(text){$('#notice').textContent=text;$('#notice').hidden=!text;}
function applyTheme(){const t=snapshot.settings.theme;document.body.dataset.theme=t==='system'?(matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light'):t;}
matchMedia('(prefers-color-scheme: dark)').addEventListener('change',applyTheme);
const formatSize=bytes=>bytes>=1024*1024?(bytes/1024/1024).toLocaleString('ru',{maximumFractionDigits:1})+' МБ':Math.max(1,Math.ceil(bytes/1024))+' КБ';
const installed=id=>snapshot.settings.installed.find(x=>x.id===id);
const newer=(a,b)=>{const x=a.split('.').map(Number),y=b.split('.').map(Number);for(let i=0;i<3;i++){if(x[i]!==y[i])return x[i]>y[i];}return false;};
const updates=()=>snapshot.catalog.apps.filter(a=>installed(a.id)&&a.release&&newer(a.release.version,installed(a.id).version));
function renderSidebar(){
  const count=updates().length;
  $('#main-nav').innerHTML=`<button class="nav-item ${route==='library'?'active':''}" data-route="library">${icon('library')}Библиотека</button><button class="nav-item ${route==='updates'?'active':''}" data-route="updates">${icon('refresh')}Обновления${count?`<span class="counter">${count}</span>`:''}</button>`;
  $('#settings-nav').innerHTML=icon('settings')+'Настройки';$('#settings-nav').classList.toggle('active',route==='settings');
  $('#add-app').innerHTML=icon('plus')+'Добавить программу';
  $('#installed-count').textContent=snapshot.settings.installed.length;
  const ids=[...snapshot.settings.sidebarOrder,...snapshot.settings.installed.map(a=>a.id),...downloads.keys()].filter((id,i,all)=>all.indexOf(id)===i&&(installed(id)||downloads.has(id)));
  $('#app-list').innerHTML=ids.length?ids.map(id=>{
    const app=snapshot.catalog.apps.find(a=>a.id===id)||{id,name:id,description:'Установленная программа',icon:'library'};
    if(downloads.has(id)){const d=downloads.get(id);return `<div class="sidebar-app downloading" data-downloading="${esc(id)}" style="--progress:${Math.round(d.progress*100)}%"><button class="nav-item" disabled>${appIcon(app)}<span class="label">${esc(app.name)}<small>${d.stage==='verify'?'Установка…':'Загрузка · '+Math.round(d.progress*100)+'%'}</small></span></button><div class="sidebar-progress"><i></i></div></div>`;}
    return `<div class="sidebar-app" draggable="true" data-id="${esc(id)}"><button class="nav-item ${activeId===id?'active':''}" data-open="${esc(id)}" title="${esc(app.description)}">${appIcon(app)}<span class="label">${esc(app.name)}</span></button><button class="drag-handle" title="Изменить порядок в настройках" aria-label="Изменить порядок ${esc(app.name)}" data-route="settings">⠿</button></div>`;
  }).join(''):'<div class="sidebar-empty">Здесь будут ваши<br>установленные программы.</div>';
}
function card(app){
  const current=installed(app.id), ready=!!app.release, update=current&&ready&&newer(app.release.version,current.version), progress=downloads.get(app.id);
  const action=progress?'Устанавливается…':update?'Обновить':current?'Открыть':ready?'Установить':'Подробнее';
  return `<article class="card" data-card="${esc(app.id)}"><div class="card-art">${artwork(app)}</div><div class="card-body"><div class="card-title">${appIcon(app)}<div><h3>${esc(app.name)}</h3><div class="category">${esc(app.category)}</div></div></div><p class="card-description"><strong>${esc(app.kind||app.category||"Программа")}</strong><span>${esc(app.description)}</span></p><div class="card-foot"><span class="package-meta version"><span>${current?'v'+esc(current.version):ready?'v'+esc(app.release.version):'Нет доступного пакета'}</span>${ready&&(!current||update)?`<span>${formatSize(app.release.size)}</span>`:''}</span><button class="card-action ${!ready&&!current?'preparing':''}" ${progress?'disabled':''} data-${current&&!update?'open':'details'}="${esc(app.id)}">${action}</button></div><button class="text-button" data-cancel="${esc(app.id)}" ${progress?'':'hidden'}>Отменить</button><div class="progress-track" ${progress?'':'hidden'}><span></span></div></div></article>`;
}
function renderLibrary(){
  const apps=snapshot.catalog.apps, mine=apps.filter(a=>installed(a.id));
  $('#page').innerHTML=`<div class="page-title"><div><h1>Библиотека</h1><p class="page-subtitle">Ваши инструменты. Одно рабочее пространство.</p></div><button class="secondary" data-catalog>${icon('plus')}Добавить</button></div>${mine.length?`<div class="cards">${mine.map(card).join('')}</div>`:`<div class="empty-library"><div class="empty-symbol">${icon('library')}</div><div><h3>Начните с нужных инструментов</h3><p>Добавьте программы из своей коллекции.<br>Они появятся здесь и на панели слева.</p></div><button class="text-button" data-catalog>Открыть каталог ${icon('arrow')}</button></div>`}<div class="section-heading"><h2>${mine.length?'В вашей коллекции':'Из вашей коллекции'}</h2><button class="text-button" data-catalog>Все программы ${icon('arrow')}</button></div><div class="cards">${apps.filter(a=>!installed(a.id)).slice(0,6).map(card).join('')||'<p class="page-subtitle">Все программы коллекции уже установлены.</p>'}</div><div class="library-note">${icon('code')}<span>Есть идея для новой утилиты?<br>Единая инструкция поможет агенту подключить её к хабу.</span><button data-prompt>Показать промпт ${icon('arrow')}</button></div>`;
}
function renderUpdates(){
  const list=updates();
  $('#page').innerHTML=`<div class="page-title"><div><h1>Обновления</h1><p class="page-subtitle">Новые версии устанавливаются только по вашей кнопке.</p></div><button class="secondary" data-refresh>${icon('refresh')}Проверить</button></div>${list.length?list.map(a=>`<div class="update-row">${appIcon(a)}<div><h3>${esc(a.name)}</h3><p>${esc(installed(a.id).version)} → ${esc(a.release.version)}</p></div><button class="secondary" data-details="${esc(a.id)}">Что нового</button></div>`).join(''):`<div class="empty-library"><div class="empty-symbol">${icon('check')}</div><div><h3>${snapshot.settings.installed.length?'Доступных обновлений нет':'Пока нечего обновлять'}</h3><p>${snapshot.settings.installed.length?'Проверка каталога найдёт новые совместимые версии.':'После установки программ новые версии будут появляться здесь.'}</p></div></div>`}<div class="settings-section"><h2>Обновление ThodHUB</h2><div class="settings-rows"><div class="settings-row"><div><h3>Версия ${esc(snapshot.hubVersion)}</h3><p>Новые версии хаба доступны на GitHub. Распакуйте новую версию в ту же папку после закрытия хаба; данные хранятся отдельно.</p></div><button class="secondary" data-hub-releases>Открыть выпуски</button></div></div></div>`;
}
function renderSettings(){
  const ids=snapshot.settings.sidebarOrder.filter(id=>installed(id));
  $('#page').innerHTML=`<div class="page-title"><div><h1>Настройки</h1><p class="page-subtitle">Только то, что нужно вашему рабочему пространству.</p></div></div><section class="settings-section"><h2>Внешний вид</h2><div class="settings-rows"><div class="settings-row"><div><h3>Оформление</h3><p>Выберите тему или следуйте настройкам Windows.</p></div><div class="theme-options">${[['light','Светлая'],['dark','Тёмная'],['system','Системная']].map(([t,label])=>`<button class="theme-option ${snapshot.settings.theme===t?'selected':''}" data-theme="${t}" aria-pressed="${snapshot.settings.theme===t}"><span class="theme-preview ${t}"></span>${label}</button>`).join('')}</div></div></div></section><section class="settings-section"><h2>Каталог и обновления</h2><div class="settings-rows"><div class="settings-row"><div><h3>Проверять при запуске</h3><p>Тихая проверка в фоне. Установка всегда остаётся за вами.</p></div><label class="switch"><input type="checkbox" id="auto-check" aria-label="Проверять обновления при запуске" ${snapshot.settings.checkOnStartup?'checked':''}><span></span></label></div><div class="settings-row"><div><h3>Источник программ</h3><p>github.com/${esc(snapshot.settings.catalogRepository||'ThodProd/ThodHUB-Registry')}</p></div><button class="secondary" data-refresh>${icon('refresh')}Обновить</button></div></div></section><section class="settings-section"><h2>Данные программ</h2><button class="secondary" data-backups>Резервные копии · экспорт и восстановление</button><div class="settings-rows"><div class="settings-row"><div><h3>Хранятся отдельно от обновлений</h3><p>${esc(snapshot.dataRoot||'%LOCALAPPDATA%\\ThodHUB\\Data')}<br>У каждой программы своя папка. Облачная копия пока не включена.</p></div><button class="secondary" data-open-data>${icon('folder')}Открыть папку</button></div></div></section>${ids.length?`<section class="settings-section"><h2>Порядок программ</h2><p class="page-subtitle">Перетаскивайте ярлыки слева или используйте кнопки здесь.</p><div class="order-list">${ids.map((id,i)=>`<div class="order-row">${appIcon(snapshot.catalog.apps.find(a=>a.id===id)||{icon:'library'})}<span>${esc(snapshot.catalog.apps.find(a=>a.id===id)?.name||id)}</span><button class="icon-button" data-move="${id}" data-step="-1" ${i===0?'disabled':''} aria-label="Переместить вверх">${icon('up')}</button><button class="icon-button" data-move="${id}" data-step="1" ${i===ids.length-1?'disabled':''} aria-label="Переместить вниз">${icon('down')}</button><button class="icon-button" data-details="${id}" aria-label="Сведения о программе">${icon('info')}</button></div>`).join('')}</div></section>`:''}<section class="settings-section"><h2>Для новых идей</h2><div class="settings-rows"><div class="settings-row"><div><h3>Инструкция для агента</h3><p>Стандарт подключения, входы, выходы и правила выпуска.</p></div><button class="secondary" data-prompt>${icon('code')}Показать промпт</button></div></div></section><p class="page-subtitle">ThodHUB ${esc(snapshot.hubVersion)} · Windows · API 1</p>`;
}
function render(){applyTheme();renderSidebar();if(route==='library')renderLibrary();else if(route==='updates')renderUpdates();else if(route==='settings')renderSettings();for(const id of downloads.keys())updateProgress(id);}
async function navigate(next){
  route=next;activeId=null;moduleGeneration++;$('#module-area').hidden=true;$('#module-frame').src='about:blank';$('#page').hidden=false;
  if(fullscreen)await setFullscreen(false);
  if(isNative) native('app.close').catch(()=>{});
  $('#breadcrumb').textContent='Рабочее пространство';render();
}
function showCatalog(){category='Все';filter='';$('#catalog-search').value='';renderCatalog();$('#catalog-dialog').showModal();setTimeout(()=>$('#catalog-search').focus(),0);}
function renderCatalog(){
  $('#filters').innerHTML=['Все',...new Set(snapshot.catalog.apps.map(a=>a.category))].map(c=>`<button class="filter ${c===category?'selected':''}" data-filter="${esc(c)}">${esc(c)}</button>`).join('');
  const list=snapshot.catalog.apps.filter(a=>(category==='Все'||a.category===category)&&(a.name+' '+a.description).toLocaleLowerCase('ru').includes(filter.toLocaleLowerCase('ru')));
  $('#catalog-results').innerHTML=list.length?`<div class="cards">${list.map(card).join('')}</div>`:'<div class="empty-results">Ничего не найдено.<br>Попробуйте другое название или категорию.</div>';
  $('#catalog-summary').textContent=`В каталоге: ${snapshot.catalog.apps.length}`;
  for(const id of downloads.keys())updateProgress(id);
}
function details(id){
  const app=snapshot.catalog.apps.find(a=>a.id===id);if(!app)return;
  const current=installed(id), update=current&&app.release&&newer(app.release.version,current.version);
  $('#details-content').innerHTML=`<button class="icon-button detail-close close-dialog" aria-label="Закрыть">${icon('close')}</button><div class="card-art">${artwork(app)}</div><div class="detail-body"><div class="eyebrow">${esc(app.category)}</div><h2 id="details-title">${esc(app.name)}</h2><p>${esc(app.description)}</p><dl class="detail-meta"><div><dt>Установлено</dt><dd>${current?'v'+esc(current.version):'Не установлено'}</dd></div><div><dt>Доступная версия</dt><dd>${app.release?'v'+esc(app.release.version):'Выпуск недоступен'}</dd></div></dl>${app.release?`<h3>${update?'Что нового':'О выпуске'}</h3><pre class="release-notes">${esc(app.release.notes||'Описание выпуска пока не добавлено.')}</pre><p>Готовый пакет из ${esc(app.repository)}. ${Math.ceil(app.release.size/1024/1024)} МБ.</p><p>Исполняемая часть программы работает с правами вашей учётной записи. Устанавливайте только доверенные программы.</p>`:'<p>Исходный проект найден. Совместимый пакет для хаба ещё не опубликован.</p>'}${app.releaseError?`<p>${esc(app.releaseError)}</p>`:''}<div class="detail-actions">${current&&!update?`<button class="primary" data-open="${id}">Открыть</button>`:app.release?`<button class="primary" data-install="${id}">${icon('download')}${update?'Обновить':'Установить'}</button>`:''}${current?`<button class="secondary" data-backups="${id}">Резервные копии</button><button class="danger" data-uninstall="${id}">Деинсталлировать</button>${current.previousVersion?`<button class="secondary" data-rollback="${id}">Вернуть ${esc(current.previousVersion)}</button>`:''}`:''}<button class="secondary close-dialog">Закрыть</button></div>${current?'<p class="page-subtitle">Удаление данных можно выбрать в следующем окне.</p>':''}</div>`;
  $('#details-dialog').showModal();
}
async function refresh(){
  if(syncing)return;syncing=true;$('#refresh').classList.add('spinning');$('#sync-label').textContent='Проверяем каталог…';
  try{snapshot=await native('catalog.refresh');notice(snapshot.catalog.apps.some(a=>a.releaseError)?'Часть выпусков проверить не удалось. Сохранённые данные доступны.':'');render();if($('#catalog-dialog').open)renderCatalog();$('#sync-label').textContent='Каталог обновлён';}
  catch(e){notice('Каталог не обновлён. '+e.message+' Установленные программы доступны без сети.');$('#sync-label').textContent='Локальный каталог';}
  finally{syncing=false;$('#refresh').classList.remove('spinning');}
}
async function install(id){
  if(downloads.has(id))return;
  downloads.set(id,{progress:0,stage:'download'});$('#details-dialog').close();render();renderCatalog();
  try{snapshot=await native('app.install',{id});notify('Программа установлена. Можно открывать.');}
  catch(e){notify(e.message);}
  finally{downloads.delete(id);render();renderCatalog();}
}
function updateProgress(id){
  const p=downloads.get(id);if(!p)return;
  const sidebar=document.querySelector('[data-downloading="'+CSS.escape(id)+'"]');if(sidebar){sidebar.style.setProperty('--progress',Math.round(p.progress*100)+'%');sidebar.querySelector('small').textContent=p.stage==='verify'?'Установка…':'Загрузка · '+Math.round(p.progress*100)+'%';}
  document.querySelectorAll('[data-card]').forEach(el=>{
    if(el.dataset.card!==id)return;
    el.querySelector('.progress-track').hidden=false;el.querySelector('.progress-track span').style.width=Math.round(p.progress*100)+'%';
    const button=el.querySelector('.card-action');button.disabled=true;button.textContent=p.stage==='verify'?'Проверка…':Math.round(p.progress*100)+'%';
  });
}
async function openApp(id){
  try{
    if(fullscreen)await setFullscreen(false);
    const result=await native('app.open',{id});moduleGeneration++;activeId=id;route='module';
    if(result.mode==='standalone'){
      document.querySelectorAll('dialog[open]').forEach(d=>d.close());
      $('#module-area').hidden=true;$('#module-frame').src='about:blank';$('#page').hidden=false;
      const app=snapshot.catalog.apps.find(a=>a.id===id)||{name:id,icon:'library',description:'',repository:''};
      $('#breadcrumb').textContent=app.name;
      $('#page').innerHTML=`<div class="page-title"><div><h1>${esc(app.name)}</h1><p class="page-subtitle">Самостоятельная программа · v${esc(result.version)}</p></div><button class="secondary" data-details="${id}">Сведения</button></div><div class="standalone-hero"><div class="standalone-art">${artwork(app)}</div><div><div class="eyebrow">ОТДЕЛЬНОЕ ОКНО</div><h2>Пространство для работы</h2><p>${esc(app.description)}</p><button class="primary" data-launch="${id}">${icon('external')}Запустить ${esc(app.name)}</button>${result.requiresElevation?'<p class="page-subtitle">Windows запросит права администратора для этой программы. Сам хаб работает без повышения прав.</p>':''}</div></div><section class="settings-section"><h2>Управление программой</h2><div class="settings-rows">${(result.actions||[]).map(a=>`<div class="settings-row"><div><h3>${esc(a.label)}</h3><p>Откроется соответствующее действие программы.</p></div><button class="secondary" data-action="${esc(a.id)}" data-app="${esc(id)}">${esc(a.label)}</button></div>`).join("")}<div class="settings-row"><div><h3>Ярлык на рабочем столе</h3><p>Запуск программы напрямую, без открытия библиотеки.</p></div><button class="secondary" data-shortcut="${id}">Создать ярлык</button></div><div class="settings-row"><div><h3>Настройки и данные</h3><p>Локальная папка этой программы в ThodHUB.</p></div><button class="secondary" data-app-data="${id}">${icon('folder')}Открыть папку</button></div><div class="settings-row"><div><h3>Удалить с компьютера</h3><p>Удалятся установленные версии и ярлык. Данные можно сохранить.</p></div><button class="danger" data-uninstall="${id}">${icon('trash')}Деинсталлировать</button></div></div></section>`;
      renderSidebar();return;
    }
    document.querySelectorAll('dialog[open]').forEach(d=>d.close());$('#page').hidden=true;$('#module-area').hidden=false;
    $('#module-frame').src=result.url+'?theme='+document.body.dataset.theme;
    const app=snapshot.catalog.apps.find(a=>a.id===id)||{name:id,icon:'library'};
    $('#module-toolbar').innerHTML=`${appIcon(app)}<strong>${esc(app.name)}</strong><span class="version">v${esc(result.version)}</span><button class="icon-button" data-fullscreen title="На весь экран (F11)" aria-label="Развернуть программу на весь экран">${icon('expand')}</button><button class="icon-button" data-details="${id}" aria-label="Сведения">${icon('info')}</button><button class="icon-button" data-route="library" aria-label="Вернуться в библиотеку">${icon('close')}</button>`;
    $('#breadcrumb').textContent=app.name;renderSidebar();
  }catch(e){notify(e.message);}
}
window.addEventListener('message', async e=>{
  if(!activeId||e.source!==$('#module-frame').contentWindow||e.data?.channel!=='thodhub:request'||typeof e.data.id!=='string'||typeof e.data.method!=='string')return;
  const source=e.source, appId=activeId, message=e.data, generation=moduleGeneration;
  try{const result=await native('module.call',{appId,method:message.method,params:message.params??null});if(generation===moduleGeneration&&activeId===appId)source.postMessage({channel:'thodhub:response',id:message.id,result},'*');}
  catch(error){if(generation===moduleGeneration&&activeId===appId)source.postMessage({channel:'thodhub:response',id:message.id,error:error.message},'*');}
});
async function setFullscreen(enabled){
  if(isNative)await native('hub.fullscreen',{enabled});
  fullscreen=enabled;document.body.classList.toggle('module-fullscreen',enabled);
  const button=$('[data-fullscreen]');if(button){button.title=enabled?'Выйти из полноэкранного режима (Esc)':'На весь экран (F11)';button.setAttribute('aria-label',button.title);}
}
function confirmRemoval(id){
  removeId=id;$('#remove-data').checked=false;$('#remove-backup').checked=true;
  const name=snapshot.catalog.apps.find(a=>a.id===id)?.name||id;
  $('#remove-description').textContent=`«${name}» будет удалена с компьютера вместе с её ярлыком. Программа останется доступной в каталоге для повторной установки.`;
  $('#remove-dialog').showModal();
}
$('#confirm-remove').onclick=async()=>{
  const id=removeId;if(!id)return;const button=$('#confirm-remove');button.disabled=true;
  try{snapshot=await native('app.uninstall',{id,deleteData:$('#remove-data').checked,backup:$('#remove-backup').checked});document.querySelectorAll('dialog[open]').forEach(d=>d.close());await navigate('library');notify('Программа и ярлык удалены.');}
  catch(e){notify(e.message);}
  finally{button.disabled=false;}
};
async function saveSettings(){
  if(!isNative){applyTheme();render();return;}
  snapshot=await native('settings.save',{theme:snapshot.settings.theme,checkOnStartup:snapshot.settings.checkOnStartup});render();
}
async function setOrder(order){
  if(isNative)await native('sidebar.order',{order});snapshot.settings.sidebarOrder=order;renderSidebar();if(route==='settings')renderSettings();
}
async function showPrompt(){
  try{$('#prompt-text').value=isNative?await native('hub.prompt'):await fetch('/docs/AGENT-PROMPT.md').then(r=>r.text());$('#prompt-dialog').showModal();}catch(e){notify(e.message);}
}
document.addEventListener('click',async e=>{
  const b=e.target.closest('button');if(!b)return;
  try{
    if(b.classList.contains('close-dialog')){b.closest('dialog').close();return;}
    if(b.dataset.route){await navigate(b.dataset.route);return;}
    if(b.hasAttribute('data-catalog')){showCatalog();return;}
    if(b.dataset.details){details(b.dataset.details);return;}
    if(b.dataset.open){await openApp(b.dataset.open);return;}
    if(b.hasAttribute('data-hub-releases')){await native('hub.releases');return;}
    if(b.dataset.cancel){await native('app.cancel',{id:b.dataset.cancel});return;}
    if(b.hasAttribute('data-backups')){await showBackups(b.dataset.backups);return;}
    if(b.dataset.launch){await native('app.launch',{id:b.dataset.launch});notify('Программа запускается в отдельном окне.');return;}
    if(b.dataset.shortcut){await native('app.shortcut',{id:b.dataset.shortcut});notify('Ярлык создан на рабочем столе.');return;}
    if(b.dataset.appData){await native('app.openData',{id:b.dataset.appData});return;}
    if(b.hasAttribute('data-fullscreen')){await setFullscreen(!fullscreen);return;}
    if(b.dataset.action){await native('app.action',{id:b.dataset.app,action:b.dataset.action});return;}
    if(b.dataset.install){await install(b.dataset.install);return;}
    if(b.dataset.filter){category=b.dataset.filter;renderCatalog();return;}
    if(b.dataset.theme){snapshot.settings.theme=b.dataset.theme;await saveSettings();return;}
    if(b.hasAttribute('data-refresh')){await refresh();return;}
    if(b.hasAttribute('data-prompt')){await showPrompt();return;}
    if(b.hasAttribute('data-open-data')){await native('hub.openData');return;}
    if(b.dataset.uninstall){confirmRemoval(b.dataset.uninstall);return;}
    if(b.dataset.rollback){const id=b.dataset.rollback;snapshot=await native('app.rollback',{id});$('#details-dialog').close();await navigate('library');notify('Предыдущая версия восстановлена.');return;}
    if(b.dataset.move){const order=[...snapshot.settings.sidebarOrder],from=order.indexOf(b.dataset.move),to=from+Number(b.dataset.step);if(to>=0&&to<order.length){[order[from],order[to]]=[order[to],order[from]];await setOrder(order);}return;}
  }catch(error){notify(error.message);}
});
$('#add-app').onclick=showCatalog;$('#settings-nav').onclick=()=>navigate('settings');$('#refresh').innerHTML=icon('refresh');$('#refresh').onclick=refresh;$('#search-icon').innerHTML=icon('search');
document.querySelectorAll('.close-dialog').forEach(b=>b.innerHTML=icon('close'));
$('#catalog-search').oninput=e=>{filter=e.target.value;renderCatalog();};
document.addEventListener('change',async e=>{if(e.target.id==='auto-check'){snapshot.settings.checkOnStartup=e.target.checked;try{await saveSettings();}catch(error){notify(error.message);}}});
$('#copy-prompt').onclick=async()=>{try{if(isNative)await native('clipboard.copy',{text:$('#prompt-text').value});else await navigator.clipboard.writeText($('#prompt-text').value);notify('Промпт скопирован.');}catch(e){notify(e.message);}};
document.addEventListener('keydown',e=>{if((e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='k'){e.preventDefault();if(!$('#catalog-dialog').open)showCatalog();else $('#catalog-search').focus();}});
document.addEventListener('keydown',e=>{if(e.key==='F11'&&activeId&&!$('#module-area').hidden){e.preventDefault();void setFullscreen(!fullscreen).catch(error=>notify(error.message));}else if(e.key==='Escape'&&fullscreen){e.preventDefault();void setFullscreen(false).catch(error=>notify(error.message));}});
$('#app-list').addEventListener('dragstart',e=>{const row=e.target.closest('.sidebar-app');if(!row)return;dragId=row.dataset.id;e.dataTransfer.effectAllowed='move';e.dataTransfer.setData('text/plain',dragId);row.classList.add('dragging');});
$('#app-list').addEventListener('dragover',e=>{const row=e.target.closest('.sidebar-app');if(!row||!dragId)return;e.preventDefault();document.querySelectorAll('.drag-over').forEach(x=>x.classList.remove('drag-over'));row.classList.add('drag-over');});
$('#app-list').addEventListener('drop',async e=>{const row=e.target.closest('.sidebar-app');if(!row||!dragId)return;e.preventDefault();const order=[...snapshot.settings.sidebarOrder],from=order.indexOf(dragId),to=order.indexOf(row.dataset.id);if(from<0||to<0)return;order.splice(from,1);order.splice(to,0,dragId);try{await setOrder(order);}catch(error){notify(error.message);}});
$('#app-list').addEventListener('dragend',()=>{dragId=null;document.querySelectorAll('.dragging,.drag-over').forEach(x=>x.classList.remove('dragging','drag-over'));});
document.querySelectorAll('dialog').forEach(d=>d.addEventListener('click',e=>{if(e.target===d){const r=d.getBoundingClientRect();if(e.clientX<r.left||e.clientX>r.right||e.clientY<r.top||e.clientY>r.bottom)d.close();}}));
async function start(){
  $('#page').innerHTML='<h1>Библиотека</h1><div class="loading-placeholder" aria-label="Загрузка библиотеки"></div>';
  try{
    if(isNative)snapshot=await native('hub.state');
    else{const response=await fetch('/registry/catalog.json');if(!response.ok)throw new Error('Не удалось загрузить каталог предпросмотра.');snapshot.catalog=await response.json();$('#foot-state').textContent='Предпросмотр интерфейса';$('#foot-state').classList.add('preview-badge');}
    render();if(isNative&&snapshot.settings.checkOnStartup)void refresh();
  }catch(e){notice(e.message);render();}
}
void start();

document.addEventListener("error",e=>{if(e.target instanceof HTMLImageElement&&e.target.dataset.fallbackIcon)e.target.outerHTML=artwork({icon:e.target.dataset.fallbackIcon});},true);
