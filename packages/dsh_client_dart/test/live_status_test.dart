import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';
HistoryEvent e(int seq,String type,Json data)=>HistoryEvent.fromJson({'seq':seq,'type':type,'data':data});
void main(){
  test('reasoning stops pulsing when output starts; request completion does not end a turn',(){
    final events=[e(1,'assistant/chunk',{'turn':1,'step':1,'chunk':{'type':'reasoning-delta','index':0,'text':'思考'}}),e(2,'assistant/chunk',{'turn':1,'step':1,'chunk':{'type':'text-delta','index':1,'text':'回复'}}),e(3,'request/phase',{'turn':1,'step':1,'phase':'completed'})];
    final rows=projectTranscript(events);expect(rows.first.streaming,isFalse);expect(rows.last.streaming,isTrue);
    final stopped=projectTranscript([...events,e(4,'turn/end',{'turn':1,'reason':{'kind':'done'}})]);
    expect(stopped.every((r)=>!r.streaming),isTrue);
    expect(projectTranscript(events,live:false).every((r)=>!r.streaming),isTrue);
  });
  test('blank name is localized and generated title remains authoritative',(){
    final row=SessionSummary.fromJson({'sessionId':'s','blank':true,'projections':{'values':{'title':'内部占位'}}});
    expect(row.displayTitle,'新会话');row.blank=false;row.title='服务生成的标题';expect(row.displayTitle,'服务生成的标题');
  });
}
