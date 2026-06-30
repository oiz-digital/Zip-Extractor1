import React from "react";
import { useGetOraclePrices } from "@workspace/api-client-react";
import { formatCurrency, timeAgo } from "@/lib/utils";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Database, Activity } from "lucide-react";

export default function Oracle() {
  const { data: prices, isLoading } = useGetOraclePrices();

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold tracking-tight flex items-center gap-2">
          <Database className="text-primary w-6 h-6" /> Oracle Price Feed
        </h2>
        <p className="text-muted-foreground">Sub-second decentralized price feeds natively integrated into L1 consensus.</p>
      </div>

      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
        {isLoading ? (
          Array.from({ length: 8 }).map((_, i) => (
            <Card key={i}>
              <CardContent className="p-4 space-y-4">
                <Skeleton className="h-6 w-24" />
                <Skeleton className="h-8 w-32" />
                <Skeleton className="h-4 w-full" />
              </CardContent>
            </Card>
          ))
        ) : (
          prices?.map((feed) => (
            <Card key={feed.pair} className="relative overflow-hidden group">
              <div className={`absolute top-0 right-0 w-16 h-16 bg-gradient-to-bl opacity-10 rounded-bl-full ${feed.change24h >= 0 ? 'from-green-500' : 'from-destructive'}`} />
              <CardContent className="p-4">
                <div className="flex justify-between items-start mb-2">
                  <h3 className="font-bold text-lg">{feed.pair}</h3>
                  <div className="flex items-center gap-1 text-xs text-muted-foreground">
                    <Activity className="w-3 h-3" /> {feed.sources} src
                  </div>
                </div>
                
                <div className="mb-4">
                  <div className="flex items-baseline gap-2">
                    <span className="text-2xl font-bold font-mono tracking-tight">{formatCurrency(feed.price, feed.price.includes('.') && parseFloat(feed.price) < 1 ? 4 : 2)}</span>
                  </div>
                  <div className={`text-sm font-mono mt-1 ${feed.change24h >= 0 ? 'text-green-500' : 'text-destructive'}`}>
                    {feed.change24h > 0 ? '+' : ''}{feed.change24h}% (24h)
                  </div>
                </div>
                
                <div className="grid grid-cols-2 gap-2 text-xs border-t border-border/50 pt-2 mt-2">
                  <div>
                    <span className="text-muted-foreground block">24h High</span>
                    <span className="font-mono">{formatCurrency(feed.high24h, 2)}</span>
                  </div>
                  <div>
                    <span className="text-muted-foreground block">24h Low</span>
                    <span className="font-mono">{formatCurrency(feed.low24h, 2)}</span>
                  </div>
                </div>
                
                <div className="mt-3 text-[10px] text-muted-foreground text-right flex justify-between items-center">
                  <span>Dev: {feed.deviation ? `${feed.deviation}%` : '0.01%'}</span>
                  <span>Updated: {timeAgo(feed.lastUpdated)}</span>
                </div>
              </CardContent>
            </Card>
          ))
        )}
      </div>
      
      {!isLoading && (!prices || prices.length === 0) && (
        <div className="text-center py-12 text-muted-foreground border border-dashed border-border rounded-lg">
          No active price feeds available.
        </div>
      )}
    </div>
  );
}
