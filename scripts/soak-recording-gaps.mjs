// Inspect adjacent indexed clips, never assume consecutive polling results are adjacent files.
export function unexplainedGaps(clips, alias, events) {
  const ordered=[...clips].sort((a,b)=>a.start_ms-b.start_ms), gaps=[];
  for(let i=1;i<ordered.length;i++){
    const previous=ordered[i-1],current=ordered[i],gapMs=current.start_ms-previous.end_ms;
    const expected=events.some(e=>e.done&&e.affected.includes(alias)&&e.started<=current.start_ms&&(e.restored||e.actionFinished||e.started)+75000>=previous.end_ms);
    if(gapMs>2000&&!expected)gaps.push({key:alias,kind:'recording-gap',gapMs,previous:previous.file,current:current.file});
  }
  return gaps;
}
