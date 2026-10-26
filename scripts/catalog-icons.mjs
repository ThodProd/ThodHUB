import fs from 'node:fs';
import vm from 'node:vm';
const source=fs.readFileSync('src/ThodHub/wwwroot/app.js','utf8');
const paths=vm.runInNewContext('('+source.match(/const paths = (\{[\s\S]*?\n\});/)[1]+')');
paths.media='<rect x="3" y="4" width="18" height="16" rx="3"/><path d="m10 8 6 4-6 4z"/>';
paths.view='<rect x="3" y="4" width="18" height="16" rx="3"/><circle cx="9" cy="9" r="2"/><path d="m4 18 6-6 4 4 3-3 4 4"/>';
const catalog=JSON.parse(fs.readFileSync('projects/ThodHUB-Registry/catalog.json','utf8').replace(/^\uFEFF/,''));
for(const app of catalog.apps){
  const key=app.id==='thodmedia'?'media':app.id==='thodview'?'view':app.icon;
  const svg=`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="15" fill="#edf2f8"/><g transform="translate(12 12) scale(1.6667)" fill="none" stroke="#42638b" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round">${paths[key]||paths.library}</g></svg>`;
  for(const root of ['registry','projects/ThodHUB-Registry']){fs.mkdirSync(root+'/assets/icons',{recursive:true});fs.writeFileSync(root+`/assets/icons/${app.id}.svg`,svg);}
  app.iconUrl=`https://raw.githubusercontent.com/ThodProd/ThodHUB-Registry/main/assets/icons/${app.id}.svg`;
}
for(const root of ['registry','projects/ThodHUB-Registry'])fs.writeFileSync(root+'/catalog.json',JSON.stringify(catalog,null,2)+'\n');
