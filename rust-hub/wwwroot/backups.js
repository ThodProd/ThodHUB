'use strict';
let backupState=null,backupSelection='',backupBusy=false,restoreEntry=null;
let backupMode='history';
async function showBackups(id='',mode='history'){
  backupMode=mode;
  if(activeId===id)await navigate('library');
  if(id)backupSelection=id;
  if(!$('#backup-dialog').open)$('#backup-dialog').showModal();
  $('#backup-content').innerHTML='<p class="backup-status">Загружаем историю копий…</p>';
  try{backupState=await native('backups.list');renderBackups();}catch(e){$('#backup-content').innerHTML=`<p class="backup-status">${esc(e.message)}</p>`;}
}
function renderBackups(){
  const state=backupState;if(!state)return;
  if(!state.apps.some(a=>a.id===backupSelection))backupSelection=state.apps[0]?.id||'thodhub-settings';
  const entries=state.entries.filter(e=>e.appId===backupSelection);
  $('#backup-content').innerHTML=`<div class="backup-body"><p class="page-subtitle" id="backup-mode-help">${backupMode==='export'?'Экспорт сохраняет базу и настройки. Выберите ZIP на компьютере или новую копию в GitHub.':backupMode==='import'?'Выберите ZIP или сохранённую копию по дате. Перед восстановлением текущие данные будут скопированы.':'Базы и настройки программы: экспорт, импорт и история копий.'}</p><div class="backup-controls"><label>Программа<select id="backup-app">${state.apps.map(a=>`<option value="${esc(a.id)}" ${a.id===backupSelection?'selected':''}>${esc(a.name)}</option>`).join('')}</select></label><button class="primary" id="backup-export">${state.connected?'Создать копию в GitHub':'Создать локальную копию'}</button><button class="secondary" id="backup-reload">Обновить</button></div><div class="backup-file-actions"><button class="secondary" id="backup-save-file">Экспорт в ZIP…</button><button class="secondary" id="backup-import-file">Импорт из ZIP…</button></div><p class="page-subtitle">Каждый экспорт создаёт новую копию с датой и временем. Старые версии не удаляются. Перед копированием закройте программу.</p>${state.warning?`<p class="backup-warning">${esc(state.warning)} Локальные копии доступны.</p>`:''}<div class="backup-history">${entries.length?entries.map((entry,index)=>`<div class="backup-row"><div><strong>${new Date(entry.createdAt).toLocaleString('ru-RU')}</strong><small>${formatSize(entry.size)} · ${entry.local&&entry.cloud?'Компьютер и GitHub':entry.cloud?'GitHub':'На компьютере'}</small></div><button class="secondary" data-restore-index="${index}">Восстановить</button></div>`).join(''):'<p class="page-subtitle">Для этой программы пока нет резервных копий.</p>'}</div><details class="backup-connection" ${state.connected?'':'open'}><summary>GitHub · ${state.connected?'подключён':'подключить приватное хранилище'}</summary><label>Приватный репозиторий<input id="backup-repo" value="${esc(state.repository)}" placeholder="имя/ThodHUB-Backups" autocomplete="off"></label><div class="backup-controls"><button class="secondary" id="backup-cli">Использовать сохранённый вход GitHub CLI</button>${state.connected?'<button class="secondary" id="backup-disconnect">Отключить</button>':''}</div><details><summary>Подключить токен репозитория</summary><p class="page-subtitle">Токен с доступом Contents: Read and write только к приватному репозиторию. Он сохраняется под защитой Windows и не входит в резервные копии. После переустановки Windows подключите GitHub заново.</p><label>Токен<input id="backup-token" type="password" autocomplete="off" placeholder="Токен не отображается после сохранения"></label><div class="backup-controls"><button class="secondary" id="backup-token-page">Создать токен на GitHub</button><button class="primary" id="backup-connect">Подключить</button></div></details></details><p class="page-subtitle">Локально: ${esc(state.localPath)}. Служебные папки cache, logs, temp, tmp и runtime не копируются. ZIP-файлы хранятся в приватном GitHub без дополнительного шифрования содержимого.</p><button class="text-button" id="backup-folder">Открыть локальные копии</button><p class="backup-status" id="backup-operation" role="status"></p></div>`;
  $('#backup-app').onchange=e=>{backupSelection=e.target.value;renderBackups();};
  $('#backup-export').onclick=()=>backupAction(async()=>{const e=await native('backups.create',{id:backupSelection});notify(e.cloud?'Копия сохранена локально и в GitHub.':'Локальная копия сохранена.');});
  $('#backup-save-file').onclick=()=>backupAction(async()=>{const result=await native('backups.exportFile',{id:backupSelection});if(!result.canceled)notify('База и настройки сохранены в ZIP.');});
  $('#backup-import-file').onclick=async()=>{let imported;await backupAction(async()=>{imported=await native('backups.importFile',{id:backupSelection});});if(imported){restoreEntry=imported;$('#restore-description').textContent=state.apps.find(a=>a.id===backupSelection)?.name+' · '+new Date(imported.createdAt).toLocaleString('ru-RU')+' · '+formatSize(imported.size);$('#restore-dialog').showModal();}};
  $('#backup-reload').onclick=()=>showBackups();
  $('#backup-cli').onclick=()=>backupAction(async()=>{backupState=await native('backups.connectCli',{repository:$('#backup-repo').value});});
  $('#backup-connect').onclick=()=>backupAction(async()=>{const token=$('#backup-token').value;$('#backup-token').value='';backupState=await native('backups.connect',{repository:$('#backup-repo').value,token});});
  $('#backup-disconnect')?.addEventListener('click',()=>backupAction(async()=>{backupState=await native('backups.disconnect');}));
  $('#backup-token-page').onclick=()=>native('backups.tokenPage').catch(e=>notify(e.message));
  $('#backup-folder').onclick=()=>native('backups.folder').catch(e=>notify(e.message));
  document.querySelectorAll('[data-restore-index]').forEach(b=>b.onclick=()=>{restoreEntry=entries[Number(b.dataset.restoreIndex)];$('#restore-description').textContent=`${state.apps.find(a=>a.id===backupSelection)?.name}: ${new Date(restoreEntry.createdAt).toLocaleString('ru-RU')}, ${formatSize(restoreEntry.size)}.`;$('#restore-dialog').showModal();});
}
async function backupAction(action){
  if(backupBusy)return;backupBusy=true;
  $('#backup-content').querySelectorAll('button,select,input').forEach(b=>b.disabled=true);
  $('#backup-operation').textContent='Выполняем… Локальная копия сохраняется до отправки в GitHub.';
  try{await action();backupState=await native('backups.list');renderBackups();}
  catch(e){notify(e.message);$('#backup-operation').textContent=e.message;$('#backup-content').querySelectorAll('button,select,input').forEach(b=>b.disabled=false);}
  finally{backupBusy=false;}
}
$('#confirm-restore').onclick=async()=>{
  if(!restoreEntry)return;const entry=restoreEntry;$('#restore-dialog').close();
  await backupAction(async()=>{snapshot=await native('backups.restore',{id:entry.appId,name:entry.name});render();notify('Настройки восстановлены. Откройте программу заново.');});
};
