package mobile

import (
	"context"
	"encoding/binary"
	"io"
	"time"

	"gvisor.dev/gvisor/pkg/tcpip/adapters/gonet"
	"gvisor.dev/gvisor/pkg/tcpip/transport/udp"
	"gvisor.dev/gvisor/pkg/waiter"
)

func (e *Engine) acceptUDP(request *udp.ForwarderRequest) {
	id:=request.ID()
	r,host,err:=e.decide(id,"udp");if err!=nil{return}
	f,ctx,err:=e.reserve(id,"udp",r,host);if err!=nil{return}
	var queue waiter.Queue
	endpoint,endpointErr:=request.CreateEndpoint(&queue);if endpointErr!=nil{e.finish(f);return}
	local:=gonet.NewUDPConn(&queue,endpoint)
	e.mu.Lock();f.local=local;e.mu.Unlock()
	go func(){
		defer e.finish(f)
		if id.LocalPort==53 {e.serveDNS(ctx,f,r,host,local);return}
		upstream,err:=e.dial(ctx,r,host,id.LocalPort,"udp");if err!=nil{e.failed(r);return};defer upstream.Close()
		stop:=context.AfterFunc(ctx,func(){local.Close();upstream.Close()});defer stop()
		done:=make(chan struct{})
		go func(){
			defer close(done)
			packet:=make([]byte,65535)
			for {local.SetReadDeadline(time.Now().Add(90*time.Second));n,err:=local.Read(packet);if err!=nil{return};written,err:=upstream.Write(packet[:n]);f.up.Add(int64(written));e.up.Add(int64(written));if err!=nil{return}}
		}()
		packet:=make([]byte,65535)
		for {upstream.SetReadDeadline(time.Now().Add(90*time.Second));n,err:=upstream.Read(packet);if err!=nil{break};written,err:=local.Write(packet[:n]);f.down.Add(int64(written));e.down.Add(int64(written));if err!=nil{break}}
		local.Close();upstream.Close();<-done
	}()
}

// DNS is carried over the already-selected node using TCP framing, so nodes
// without UDP relay support can still resolve the tunnel's DNS requests.
func (e *Engine) serveDNS(ctx context.Context,f *flow,r route,host string,local io.ReadWriteCloser){
	stop:=context.AfterFunc(ctx,func(){local.Close()});defer stop()
	for {
		packet:=make([]byte,4096)
		if timed,ok:=local.(interface{SetReadDeadline(time.Time)error});ok{timed.SetReadDeadline(time.Now().Add(30*time.Second))}
		n,err:=local.Read(packet);if err!=nil{return};if n<12{continue};query:=packet[:n]
		f.up.Add(int64(n));e.up.Add(int64(n))
		queryCtx,cancel:=context.WithTimeout(ctx,8*time.Second)
		remote,err:=e.dial(queryCtx,r,host,53,"tcp")
		if err!=nil{cancel();e.failed(r);continue}
		closeOnTimeout:=context.AfterFunc(queryCtx,func(){remote.Close()})
		framed:=make([]byte,2+len(query));binary.BigEndian.PutUint16(framed,uint16(len(query)));copy(framed[2:],query)
		_,err=remote.Write(framed)
		var length [2]byte
		if err==nil{_,err=io.ReadFull(remote,length[:])}
		var response []byte
		if err==nil {size:=int(binary.BigEndian.Uint16(length[:]));if size>=12 {response=make([]byte,size);_,err=io.ReadFull(remote,response)} }
		remote.Close();closeOnTimeout();cancel()
		if err!=nil||len(response)<12||response[0]!=query[0]||response[1]!=query[1]{continue}
		e.observeDNS(query,response)
		written,err:=local.Write(response);f.down.Add(int64(written));e.down.Add(int64(written));if err!=nil{return}
	}
}
