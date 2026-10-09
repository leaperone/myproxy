package mobile

import (
	"context"
	"io"
	"net"
	"net/http"
	"sync"
	"time"
)

// Probe checks node egress without adding internal health checks to the user's
// connection list. Concurrent requests coalesce into one bounded pass.
func (e *Engine) Probe() {
	if e.closed.Load() || !e.probeBusy.CompareAndSwap(false, true) {
		return
	}
	go func() {
		defer e.probeBusy.Store(false)
		jobs := make(chan node)
		var workers sync.WaitGroup
		for i := 0; i < 2; i++ {
			workers.Add(1)
			go func() {
				defer workers.Done()
				for n := range jobs {
					e.checkNode(n)
				}
			}()
		}
		for _, n := range e.nodes {
			select {
			case <-e.ctx.Done():
				close(jobs)
				workers.Wait()
				return
			case jobs <- n:
			}
		}
		close(jobs)
		workers.Wait()
	}()
}

func (e *Engine) checkNode(n node) {
	e.mu.Lock()
	if e.closed.Load() || e.probing[n.Tag] || len(e.probing) >= 4 { e.mu.Unlock(); return }
	e.probing[n.Tag] = true
	e.mu.Unlock()
	defer func(){ e.mu.Lock(); delete(e.probing,n.Tag); e.mu.Unlock() }()
	for attempt := 0; attempt < 2; attempt++ {
		ctx, cancel := context.WithTimeout(e.ctx, 5*time.Second)
		r := route{Action: "proxy", Tag: &n.Tag, Node: &n.Name}
		transport := &http.Transport{DisableKeepAlives: true, DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
			return e.dial(ctx, r, "www.gstatic.com", 443, "tcp")
		}, TLSHandshakeTimeout: 3*time.Second}
		client := &http.Client{Transport:transport,Timeout:5*time.Second,CheckRedirect:func(*http.Request,[]*http.Request)error{return http.ErrUseLastResponse}}
		request,_ := http.NewRequestWithContext(ctx,"GET","https://www.gstatic.com/generate_204",nil)
		start := time.Now()
		response,err := client.Do(request)
		if err==nil { io.Copy(io.Discard,io.LimitReader(response.Body,1024)); response.Body.Close() }
		failed := err!=nil || response==nil || response.StatusCode!=204
		delay := time.Since(start).Milliseconds(); if failed { delay = -1 }
		transport.CloseIdleConnections();cancel()
		if e.closed.Load() {return}
		e.policy.Health(n.Name,delay,failed)
		if !failed {return}
		if attempt==0 { select {case <-e.ctx.Done():return;case <-time.After(300*time.Millisecond):} }
	}
}
