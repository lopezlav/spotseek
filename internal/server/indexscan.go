// AI-assisted code: share index scans and report when local links are ready.
package server

import "log"

type indexScanFlight struct {
	done  chan struct{}
	root  string
	count int
	err   error
}

func (s *Server) startIndexScan() *indexScanFlight {
	s.scanMu.Lock()
	defer s.scanMu.Unlock()
	root := s.index.Root()
	previous := s.scanFlight
	if previous != nil && previous.root == root {
		return previous
	}
	flight := &indexScanFlight{done: make(chan struct{}), root: root}
	s.scanFlight = flight
	s.scanWG.Add(1)
	go func() {
		defer s.scanWG.Done()
		if previous != nil {
			<-previous.done
		}
		flight.count, flight.err = s.index.Scan(logf)
		if flight.err != nil {
			log.Printf("index scan: %v", flight.err)
		}
		s.scanMu.Lock()
		if s.scanFlight == flight {
			s.scanFlight = nil
		}
		close(flight.done)
		s.scanMu.Unlock()
	}()
	return flight
}

func (s *Server) Wait() {
	s.manager.Wait()
	s.scanWG.Wait()
	s.backgroundWG.Wait()
	s.metadata.wg.Wait()
}

// End AI-assisted code.
